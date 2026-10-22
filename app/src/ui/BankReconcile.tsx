/**
 * The reconcile panel for one bank line: what the line is, the suggestions
 * the server ranked, the entry it would post, and the actions. Match posts the
 * entry (or links an entry already posted), Create rule saves a rule from the
 * line, Ignore leaves the line out on purpose. The remittance is shown as text
 * and never acted on.
 */
import { useState } from 'preact/hooks';
import {
  bankRulesList,
  bankRuleCreate,
  bankTransactionIgnore,
  bankTransactionLink,
  bankTransactionPost,
  isNeedsApproval,
  type AccountView,
  type Suggestion,
  type TransactionView,
} from '../api/bank';
import { useAsync } from '../hooks';
import { useSession } from '../session';
import { distinctiveWord, formatIban, journalPreview, lineStatus, REASON_KEYS, type PreviewLine } from '../lib/bank';
import { t, type MessageKey } from '../i18n';
import { ErrorBanner, Money } from './common';
import { chartLabel, counterpartAccounts, accountOptions, SignedAmount, StatusChip, useChart } from './BankShared';
import { Icon } from './icons';
import { Select } from './Select';
import { accountNeedsUnit, colourOf } from '../lib/units';

type Choice = { kind: 'suggestion'; index: number } | { kind: 'manual' };
type Notice = { kind: 'ok' | 'warn'; text: string; href?: string; hrefLabel?: string };

const KIND_KEYS: Record<Suggestion['kind'], MessageKey> = {
  rule: 'bank.kind_rule',
  bill: 'bank.kind_bill',
  entry: 'bank.kind_entry',
};

export function BankReconcile({ account, line, onChanged }: { account: AccountView; line: TransactionView; onChanged: () => void }) {
  const { client, toast, unit: scopeUnit, units } = useSession();
  const unitsEnabled = units.length > 0;
  const chart = useChart();
  const rules = useAsync(() => bankRulesList(client).then((r) => r.rules), [client.baseUrl, client.apiKey]);
  const [choice, setChoice] = useState<Choice | null>(line.suggestions.length > 0 ? { kind: 'suggestion', index: 0 } : null);
  const [manualAccount, setManualAccount] = useState('');
  // The business unit a manual or rule match posts to: the switcher's unit by default.
  const [matchUnit, setMatchUnit] = useState(scopeUnit ?? '');
  const [ruleUnit, setRuleUnit] = useState(scopeUnit ?? '');
  const [unitProblem, setUnitProblem] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [ruleOpen, setRuleOpen] = useState(false);
  const [ruleName, setRuleName] = useState('');
  const [ruleContains, setRuleContains] = useState('');
  const [ruleIban, setRuleIban] = useState(false);
  const [ruleAccount, setRuleAccount] = useState('');
  const [ruleProblem, setRuleProblem] = useState<MessageKey | null>(null);

  const status = lineStatus(line);
  const flagged = line.flags.includes('remittance-reads-like-instruction');
  const accounts = chart.data ?? [];
  const counterparts = counterpartAccounts(accounts);
  const currency = line.amount.currency || account.currency;
  const amount = BigInt(line.amount.minor);
  const open = status === 'unmatched' || status === 'suggested';
  const suggestion = choice?.kind === 'suggestion' ? line.suggestions[choice.index] : undefined;

  /** The account the other side of the entry goes to, when it is known from the choice. */
  const counterpartNumber = (): string | null => {
    if (!choice) return null;
    if (choice.kind === 'manual') return manualAccount || null;
    if (!suggestion) return null;
    if (suggestion.kind === 'rule') return suggestion.account_number;
    if (suggestion.kind === 'bill') return null;
    return null;
  };

  const counterpartLabel = (): string => {
    if (suggestion?.kind === 'bill') return t('bank.payables_account');
    const number = counterpartNumber();
    if (!number) return t('bank.preview_none');
    const found = accounts.find((a) => a.number === number);
    return found ? `${number} ${chartLabel(found)}` : number;
  };

  const lines: PreviewLine[] =
    suggestion?.kind === 'entry'
      ? []
      : journalPreview({
          bankAccount: account.ledger_account_number,
          counterparty: counterpartNumber() ?? '',
          amountMinor: amount,
          currency,
        });

  // A rule without an account cannot post on its own: the person picks the other account instead.
  const matchable =
    choice !== null && (choice.kind === 'manual' ? manualAccount !== '' : suggestion !== undefined && (suggestion.kind !== 'rule' || !!suggestion.account_number));
  const canMatch = !busy && matchable;

  const afterWrite = (result: unknown, done: string) => {
    if (isNeedsApproval(result)) {
      setNotice({ kind: 'warn', text: t('bank.notice_approval'), href: `#/approvals/${encodeURIComponent(result.approval.approval_id)}`, hrefLabel: t('bank.notice_approval_link') });
      return;
    }
    toast(done);
    onChanged();
  };

  // The counterpart account a match or rule posts to, and whether it needs a unit (income and expense lines do, once the company has units).
  const postsNeedUnit = (number: string | null) => unitsEnabled && accountNeedsUnit(accounts.find((a) => a.number === number));
  const matchAccount = choice?.kind === 'manual' ? manualAccount : suggestion?.kind === 'rule' ? suggestion.account_number : null;
  const matchMissingUnit = !!matchAccount && postsNeedUnit(matchAccount) && matchUnit === '';
  const ruleMissingUnit = postsNeedUnit(ruleAccount) && ruleUnit === '';

  const match = async () => {
    if (!choice || busy) return;
    if (matchMissingUnit) {
      setUnitProblem(true);
      return;
    }
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      if (choice.kind === 'manual') {
        const result = await bankTransactionPost(client, line.id, {
          counterparty_account: manualAccount,
          ...(matchUnit === '' ? {} : { unit: matchUnit }),
        });
        afterWrite(result, t('bank.toast_matched'));
      } else if (suggestion?.kind === 'entry') {
        const result = await bankTransactionLink(client, line.id, suggestion.target_id);
        afterWrite(result, t('bank.toast_linked'));
      } else if (suggestion?.kind === 'bill') {
        const result = await bankTransactionPost(client, line.id, { bill_id: suggestion.target_id });
        afterWrite(result, t('bank.toast_matched'));
      } else if (suggestion?.kind === 'rule' && suggestion.account_number) {
        const result = await bankTransactionPost(client, line.id, {
          counterparty_account: suggestion.account_number,
          ...(matchUnit === '' ? {} : { unit: matchUnit }),
        });
        afterWrite(result, t('bank.toast_matched'));
      }
    } catch (err) {
      setError(err);
    } finally {
      setBusy(false);
    }
  };

  const ignore = async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      await bankTransactionIgnore(client, line.id);
      toast(t('bank.toast_ignored'));
      onChanged();
    } catch (err) {
      setError(err);
    } finally {
      setBusy(false);
    }
  };

  const openRule = () => {
    const word = distinctiveWord(line.remittance, line.counterparty_iban);
    setRuleName(word);
    setRuleContains(word);
    setRuleIban(false);
    setRuleAccount(counterpartNumber() ?? '');
    setRuleProblem(null);
    setRuleOpen(true);
  };

  const saveRule = async (event: Event) => {
    event.preventDefault();
    setRuleProblem(null);
    const contains = ruleContains.trim();
    const iban = ruleIban && line.counterparty_iban ? line.counterparty_iban : null;
    if (ruleName.trim() === '') return setRuleProblem('bank.rule_name_required');
    if (contains === '' && iban === null) return setRuleProblem('bank.rule_needs_condition');
    if (ruleAccount === '') return setRuleProblem('bank.rule_account_required');
    if (ruleMissingUnit) return setRuleProblem('units.err_line');
    setBusy(true);
    setError(null);
    try {
      const result = await bankRuleCreate(client, {
        name: ruleName.trim(),
        conditions: { contains: contains === '' ? null : contains, counterparty_iban: iban, amount_min: null, amount_max: null },
        account_number: ruleAccount,
        unit: ruleUnit === '' ? null : ruleUnit,
      });
      setRuleOpen(false);
      rules.reload();
      if (isNeedsApproval(result)) {
        setNotice({ kind: 'warn', text: t('bank.notice_approval'), href: `#/approvals/${encodeURIComponent(result.approval.approval_id)}`, hrefLabel: t('bank.notice_approval_link') });
      } else if (!result.enabled) {
        setNotice({ kind: 'warn', text: t('bank.notice_rule_waits') });
      } else {
        toast(t('bank.toast_rule_created'));
        onChanged();
      }
    } catch (err) {
      setError(err);
    } finally {
      setBusy(false);
    }
  };

  const reasonWords = (codes: string[]) =>
    codes.map((code) => (REASON_KEYS[code] ? t(REASON_KEYS[code]) : null)).filter((word): word is string => word !== null);

  return (
    <div class="bank-reconcile">
      <dl class="bank-facts">
        <div>
          <dt>{t('bank.col_date')}</dt>
          <dd class="mono">{line.booking_date}</dd>
        </div>
        <div>
          <dt>{t('bank.col_amount')}</dt>
          <dd>
            <SignedAmount minor={line.amount.minor} currency={currency} />
          </dd>
        </div>
        <div>
          <dt>{t('bank.col_counterparty')}</dt>
          <dd>
            {line.counterparty_name || t('bank.no_counterparty')}
            {line.counterparty_iban && <span class="mono bank-fact-iban">{formatIban(line.counterparty_iban)}</span>}
          </dd>
        </div>
        <div>
          <dt>{t('bank.col_status')}</dt>
          <dd>
            <StatusChip status={status} />
          </dd>
        </div>
      </dl>

      <blockquote class="bank-remit">
        <span class="sr-only">{t('bank.remit_label')}: </span>
        {line.remittance || t('bank.no_remittance')}
      </blockquote>
      <p class="hint">{t('bank.remit_is_data')}</p>

      {flagged && (
        <div class="banner human bank-flag-banner" role="note">
          <Icon name="info" />
          <span class="grow">{t('bank.flag_instruction')}</span>
        </div>
      )}

      {status === 'matched' && line.matched && (
        <p class="result ok bank-matched">
          <Icon name="check" />
          <span>
            {t('bank.matched_to', { kind: line.matched.kind === 'bill' ? t('bank.kind_bill') : t('bank.kind_entry') })}{' '}
            <a class="mono" href={line.matched.kind === 'bill' ? `#/bills/${encodeURIComponent(line.matched.id)}` : `#/journal/${encodeURIComponent(line.matched.id)}`}>
              {line.matched.id}
            </a>
          </span>
        </p>
      )}
      {status === 'ignored' && <p class="note">{t('bank.ignored_note')}</p>}

      {open && (
        <>
          {line.suggestions.length > 0 ? (
            <fieldset class="bank-suggestions">
              <legend class="caption">{t('bank.suggestions')}</legend>
              {line.suggestions.map((item, index) => {
                const words = reasonWords(item.reasons);
                const checked = choice?.kind === 'suggestion' && choice.index === index;
                const targetName = item.kind === 'rule' ? rules.data?.find((r) => r.id === item.target_id)?.name : undefined;
                return (
                  <label class={`bank-suggestion${checked ? ' selected' : ''}`} key={`${item.kind}-${item.target_id}`}>
                    <input type="radio" name="bank-suggestion" checked={checked} onChange={() => setChoice({ kind: 'suggestion', index })} />
                    <span class="bank-suggestion-body">
                      <span class="bank-suggestion-head">
                        <strong>{t(KIND_KEYS[item.kind])}</strong>
                        <span class="mono muted">{targetName ?? item.target_id}</span>
                      </span>
                      <span class="bank-score-row">
                        <span
                          class="bank-score"
                          role="meter"
                          aria-label={t('bank.score_label')}
                          aria-valuemin={0}
                          aria-valuemax={100}
                          aria-valuenow={item.score}
                          aria-valuetext={`${item.score}%`}
                        >
                          <span style={{ inlineSize: `${Math.min(100, Math.max(0, item.score))}%` }} />
                        </span>
                        <span class="mono bank-score-num">{item.score}%</span>
                      </span>
                      {words.length > 0 && <span class="bank-reasons">{words.join(' · ')}</span>}
                    </span>
                  </label>
                );
              })}
              <label class={`bank-suggestion${choice?.kind === 'manual' ? ' selected' : ''}`}>
                <input type="radio" name="bank-suggestion" checked={choice?.kind === 'manual'} onChange={() => setChoice({ kind: 'manual' })} />
                <span class="bank-suggestion-body">
                  <span class="bank-suggestion-head">
                    <strong>{t('bank.choose_other')}</strong>
                  </span>
                </span>
              </label>
            </fieldset>
          ) : (
            <p class="note">{t('bank.no_suggestions')}</p>
          )}

          {(choice?.kind === 'manual' || (choice?.kind === 'suggestion' && suggestion?.kind === 'rule' && !suggestion.account_number)) && (
            <div class="field">
              <span id="bank-manual-label">{t('bank.counterparty_account')}</span>
              <Select
                id="bank-manual-account"
                labelledBy="bank-manual-label"
                value={manualAccount}
                placeholder={t('bank.counterparty_choose')}
                emptyLabel={t('bank.counterparty_choose')}
                options={accountOptions(counterparts)}
                onChange={(value) => {
                  setManualAccount(value);
                  setUnitProblem(false);
                }}
              />
            </div>
          )}

          {unitsEnabled && matchAccount && (
            <div class="field">
              <span id="bank-match-unit-label">{t('units.field')}</span>
              <Select
                id="bank-match-unit"
                labelledBy="bank-match-unit-label"
                value={matchUnit}
                placeholder={t('units.choose_unit')}
                invalid={unitProblem && matchMissingUnit}
                options={units.map((item) => ({ value: item.code, label: item.code, hint: item.name, swatch: colourOf(item) }))}
                searchable={false}
                onChange={(value) => {
                  setMatchUnit(value);
                  setUnitProblem(false);
                }}
              />
              <span class={unitProblem && matchMissingUnit ? 'field-error' : 'hint'}>
                {unitProblem && matchMissingUnit ? t('units.err_line') : t('units.bank_hint')}
              </span>
            </div>
          )}

          {suggestion?.kind === 'entry' ? (
            <p class="bank-entry-link">
              {t('bank.entry_link_note')}{' '}
              <a class="mono" href={`#/journal/${encodeURIComponent(suggestion.target_id)}`}>
                {suggestion.target_id}
              </a>
            </p>
          ) : (
            choice && (
              <section class="bank-preview-block" aria-labelledby="bank-preview-title">
                <h3 id="bank-preview-title" class="caption">
                  {t('bank.journal_preview')}
                </h3>
                <table class="journal-preview">
                  <thead>
                    <tr>
                      <th scope="col">{t('bank.preview_account')}</th>
                      <th scope="col" class="num">{t('bank.preview_debit')}</th>
                      <th scope="col" class="num">{t('bank.preview_credit')}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {lines.map((preview, index) => (
                      <tr key={index}>
                        <td>
                          {preview.account === account.ledger_account_number ? (
                            <>
                              <span class="mono">{preview.account}</span> {t('bank.preview_bank_side')}
                            </>
                          ) : (
                            counterpartLabel()
                          )}
                        </td>
                        <td class="num">{preview.side === 'debit' ? <Money minor={preview.minor} currency={preview.currency} /> : ''}</td>
                        <td class="num">{preview.side === 'credit' ? <Money minor={preview.minor} currency={preview.currency} /> : ''}</td>
                      </tr>
                    ))}
                    {lines.length === 0 && (
                      <tr>
                        <td colSpan={3} class="muted">
                          {t('bank.preview_none')}
                        </td>
                      </tr>
                    )}
                  </tbody>
                </table>
              </section>
            )
          )}

          <div class="row-actions bank-actions">
            <button type="button" class="btn btn-primary" onClick={() => void match()} disabled={!canMatch} aria-busy={busy ? 'true' : undefined}>
              {suggestion?.kind === 'entry' ? t('bank.action_link') : t('bank.action_match')}
            </button>
            <button type="button" class="btn" onClick={openRule} disabled={busy || ruleOpen}>
              {t('bank.action_create_rule')}
            </button>
            <button type="button" class="btn btn-ghost" onClick={() => void ignore()} disabled={busy}>
              {t('bank.action_ignore')}
            </button>
          </div>

          {ruleOpen && (
            <form class="bank-rule-form" onSubmit={saveRule} aria-labelledby="bank-rule-title" noValidate>
              <h3 id="bank-rule-title" class="caption">
                {t('bank.rule_title')}
              </h3>
              <label class="field">
                <span>{t('bank.rule_name')}</span>
                <input class="input" type="text" value={ruleName} onInput={(e) => setRuleName((e.target as HTMLInputElement).value)} />
              </label>
              <label class="field">
                <span>{t('bank.rule_contains')}</span>
                <input class="input" type="text" value={ruleContains} onInput={(e) => setRuleContains((e.target as HTMLInputElement).value)} />
                <span class="hint">{t('bank.rule_contains_hint')}</span>
              </label>
              {line.counterparty_iban && (
                <label class="check">
                  <input type="checkbox" checked={ruleIban} onChange={(e) => setRuleIban((e.target as HTMLInputElement).checked)} />
                  {t('bank.rule_also_iban', { iban: formatIban(line.counterparty_iban) })}
                </label>
              )}
              <div class="field">
                <span id="bank-rule-account-label">{t('bank.rule_account')}</span>
                <Select
                  id="bank-rule-account"
                  labelledBy="bank-rule-account-label"
                  value={ruleAccount}
                  placeholder={t('bank.counterparty_choose')}
                  emptyLabel={t('bank.counterparty_choose')}
                  options={accountOptions(counterparts)}
                  onChange={setRuleAccount}
                />
              </div>
              {unitsEnabled && (
                <div class="field">
                  <span id="bank-rule-unit-label">{t('units.field')}</span>
                  <Select
                    id="bank-rule-unit"
                    labelledBy="bank-rule-unit-label"
                    value={ruleUnit}
                    placeholder={t('units.choose_unit')}
                    invalid={ruleProblem === 'units.err_line' && ruleMissingUnit}
                    options={units.map((item) => ({ value: item.code, label: item.code, hint: item.name, swatch: colourOf(item) }))}
                    searchable={false}
                    onChange={setRuleUnit}
                  />
                  <span class="hint">{t('units.bank_hint')}</span>
                </div>
              )}
              <p class="hint">{client.apiKey ? t('bank.rule_waits_note') : t('bank.rule_applies_note')}</p>
              {ruleProblem && (
                <p class="field-error" role="alert">
                  {t(ruleProblem)}
                </p>
              )}
              <div class="form-actions">
                <button type="button" class="btn btn-ghost" onClick={() => setRuleOpen(false)}>
                  {t('common.cancel')}
                </button>
                <button type="submit" class="btn btn-primary" disabled={busy}>
                  {t('bank.rule_save')}
                </button>
              </div>
            </form>
          )}
        </>
      )}

      {notice && (
        <p class={`result ${notice.kind === 'ok' ? 'ok' : 'warn'} bank-notice`} role="status">
          <Icon name="info" />
          <span>
            {notice.text}
            {notice.href && (
              <>
                {' '}
                <a href={notice.href}>{notice.hrefLabel}</a>
              </>
            )}
          </span>
        </p>
      )}
      {error !== null && <ErrorBanner error={error} />}
    </div>
  );
}
