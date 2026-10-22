/**
 * Bank rules: what each rule matches, the account it posts to, and whether it
 * applies. A rule an agent proposed waits until a person enables it here.
 */
import { useState } from 'preact/hooks';
import { bankRuleEnable, bankRulesList, type RuleView } from '../api/bank';
import { useAsync } from '../hooks';
import { useSession } from '../session';
import { conditionParts } from '../lib/bank';
import { t } from '../i18n';
import { Empty, ErrorBanner, Marker, PageHead, Skeleton } from './common';
import { BankTabs, chartLabel, useChart } from './BankShared';
import { Icon } from './icons';

export function BankRules() {
  const { client, toast } = useSession();
  const person = !client.apiKey;
  const rules = useAsync(() => bankRulesList(client).then((r) => r.rules), [client.baseUrl, client.apiKey]);
  const chart = useChart();
  const [busyId, setBusyId] = useState<string | null>(null);
  const [error, setError] = useState<unknown>(null);
  const list = rules.data ?? [];

  const enable = async (rule: RuleView) => {
    setBusyId(rule.id);
    setError(null);
    try {
      await bankRuleEnable(client, rule.id);
      toast(t('bank.toast_rule_enabled'));
      rules.reload();
    } catch (err) {
      setError(err);
    } finally {
      setBusyId(null);
    }
  };

  const accountLabelFor = (number: string) => {
    const found = chart.data?.find((account) => account.number === number);
    return found ? `${number} ${chartLabel(found)}` : number;
  };

  return (
    <>
      <PageHead title={t('bank.rules_title')}>{t('bank.rules_intro')}</PageHead>
      <BankTabs current="rules" />

      {error !== null && <ErrorBanner error={error} />}
      {rules.loading && rules.data === undefined && <Skeleton rows={3} label={t('bank.loading')} />}
      {rules.error !== undefined && rules.error !== null && <ErrorBanner error={rules.error} onRetry={rules.reload} />}
      {rules.data !== undefined && list.length === 0 && <Empty title={t('bank.rules_empty_title')}>{t('bank.rules_empty_body')}</Empty>}

      {list.length > 0 && (
        <div class="panel">
          <div class="table-wrap">
            <table class="ledger bank-rules">
              <caption class="sr-only">{t('bank.rules_title')}</caption>
              <thead>
                <tr>
                  <th scope="col">{t('bank.rule_name')}</th>
                  <th scope="col">{t('bank.rule_conditions')}</th>
                  <th scope="col">{t('bank.rule_account')}</th>
                  <th scope="col">{t('bank.rule_status')}</th>
                  <th scope="col">
                    <span class="sr-only">{t('bank.rule_actions')}</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {list.map((rule) => {
                  const words = conditionParts(rule.conditions).map((part) => {
                    switch (part.kind) {
                      case 'contains':
                        return t('bank.rule_part_contains', { text: part.text });
                      case 'iban':
                        return t('bank.rule_part_iban', { iban: part.text });
                      case 'amount-min':
                        return t('bank.rule_part_min', { amount: part.minor.toString() });
                      case 'amount-max':
                        return t('bank.rule_part_max', { amount: part.minor.toString() });
                    }
                  });
                  return (
                    <tr key={rule.id}>
                      <td class="lead">{rule.name}</td>
                      <td class="bank-rule-conditions">{words.length > 0 ? words.join(' · ') : t('bank.rule_no_conditions')}</td>
                      <td>{accountLabelFor(rule.account_number)}</td>
                      <td>
                        <span class="bank-rule-status">
                          <span class={`chip ${rule.enabled ? 'status-approved' : ''}`}>{rule.enabled ? t('bank.rule_on') : t('bank.rule_proposed')}</span>
                          <Marker by={rule.proposed_by.startsWith('person:') ? 'person' : 'agent'} />
                        </span>
                        {!rule.enabled && <span class="hint bank-rule-wait">{t('bank.rule_waits')}</span>}
                      </td>
                      <td class="bank-rule-action">
                        {person && !rule.enabled && (
                          <button type="button" class="btn btn-sm btn-human" onClick={() => void enable(rule)} disabled={busyId !== null} aria-busy={busyId === rule.id ? 'true' : undefined}>
                            <Icon name="check" />
                            {busyId === rule.id ? t('common.working') : t('bank.rule_enable')}
                          </button>
                        )}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </div>
      )}
    </>
  );
}
