/**
 * The chart of accounts as the Accounts screens use it: the five classes in
 * their order, the parent/child tree, search and status filters, and the
 * validation the account form shows inline. Pure functions, no I/O: the
 * server stays the authority (it refuses the same things again).
 */
import type { Account } from '../api/types';

export const ACCOUNT_TYPES = ['asset', 'liability', 'equity', 'income', 'expense'] as const;
export type AccountType = (typeof ACCOUNT_TYPES)[number];

export const MAX_NAME_LENGTH = 200;
export const MAX_TAGS = 16;
/** An account number: 2 to 10 ASCII digits. Leading zeros are allowed. */
export const NUMBER_PATTERN = /^[0-9]{2,10}$/;
/** A tag: lowercase kebab-case, such as "bank" or "vat-input". */
export const TAG_PATTERN = /^[a-z0-9]+(-[a-z0-9]+)*$/;
/** An ISO 4217 code, as the form takes it (three capital letters). */
export const CURRENCY_PATTERN = /^[A-Z]{3}$/;

/** The name shown for an account: the custom name when it has one, else the translated template name. */
export function accountLabel(account: Pick<Account, 'name_key' | 'display_name'>, fallback: (nameKey: string) => string): string {
  const custom = account.display_name?.trim();
  return custom ? custom : fallback(account.name_key);
}

/** How the filter shows accounts: active (not archived), archived only, or both. */
export type StatusFilter = 'active' | 'archived' | 'all';

export interface TreeRow {
  account: Account;
  /** 0 for a root of the chart; children are one deeper than their parent. */
  depth: number;
  /** Whether any account names this one as its parent (a heading), whatever its status. */
  hasChildren: boolean;
}

export interface TypeGroup {
  type: AccountType;
  rows: TreeRow[];
}

/**
 * The chart grouped by class, in the order Assets, Liabilities, Equity,
 * Income, Expenses. Within a class the tree is walked depth-first, children
 * ordered by number. An account whose parent is not in the list (or whose
 * parent is of another class) is shown as a root, so nothing disappears.
 * Every class appears, even when it has no accounts.
 */
export function buildChart(accounts: Account[]): TypeGroup[] {
  const byNumber = new Map(accounts.map((account) => [account.number, account]));
  const childrenOf = new Map<string, Account[]>();
  const roots = new Map<AccountType, Account[]>();
  for (const account of [...accounts].sort(byNumberAsc)) {
    const parent = account.parent !== null ? byNumber.get(account.parent) : undefined;
    const sameClass = parent !== undefined && parent.account_type === account.account_type;
    if (parent && sameClass) {
      childrenOf.set(parent.number, [...(childrenOf.get(parent.number) ?? []), account]);
    } else {
      const type = asClass(account.account_type);
      if (type) roots.set(type, [...(roots.get(type) ?? []), account]);
    }
  }

  const seen = new Set<string>();
  const walk = (account: Account, depth: number, out: TreeRow[]) => {
    if (seen.has(account.number)) return;
    seen.add(account.number);
    const children = childrenOf.get(account.number) ?? [];
    out.push({ account, depth, hasChildren: children.length > 0 });
    for (const child of children) walk(child, depth + 1, out);
  };

  return ACCOUNT_TYPES.map((type) => {
    const rows: TreeRow[] = [];
    for (const root of roots.get(type) ?? []) walk(root, 0, rows);
    return { type, rows };
  });
}

function byNumberAsc(a: Account, b: Account): number {
  return a.number < b.number ? -1 : a.number > b.number ? 1 : 0;
}

function asClass(value: string): AccountType | null {
  return (ACCOUNT_TYPES as readonly string[]).includes(value) ? (value as AccountType) : null;
}

/** Whether the account is archived. */
export function matchesStatus(account: Account, status: StatusFilter): boolean {
  if (status === 'all') return true;
  return status === 'archived' ? account.archived : !account.archived;
}

/**
 * Filters the grouped chart. An account stays when it matches the status and
 * the search (number, label or tag, case-insensitive); its ancestors stay too,
 * so a match keeps its place in the tree. Classes with nothing left are dropped.
 */
export function filterChart(
  groups: TypeGroup[],
  options: { query: string; status: StatusFilter; label: (account: Account) => string },
): TypeGroup[] {
  const needle = options.query.trim().toLowerCase();
  const parents = new Map<string, string | null>();
  for (const group of groups) for (const row of group.rows) parents.set(row.account.number, row.account.parent);

  const keep = new Set<string>();
  for (const group of groups) {
    for (const { account } of group.rows) {
      if (!matchesStatus(account, options.status)) continue;
      const hit =
        needle === '' ||
        account.number.toLowerCase().includes(needle) ||
        options.label(account).toLowerCase().includes(needle) ||
        account.tags.some((tag) => tag.includes(needle));
      if (!hit) continue;
      // Keep the match and its ancestors; the walk stops at a root or a repeat.
      let current: string | null | undefined = account.number;
      while (current && !keep.has(current)) {
        keep.add(current);
        current = parents.get(current) ?? null;
      }
    }
  }

  return groups
    .map((group) => ({ type: group.type, rows: group.rows.filter((row) => keep.has(row.account.number)) }))
    .filter((group) => group.rows.length > 0);
}

/** The balance sum of an account, grouped by currency, zero rows dropped. */
export function nonZeroBalances(account: Account): Account['balances'] {
  return account.balances.filter((balance) => balance.minor !== '0' && balance.minor !== '-0');
}

/** Whether the account has any posted amount. An archive is refused while it does. */
export function hasPostings(account: Account): boolean {
  return nonZeroBalances(account).length > 0;
}

/** Splits the tags field on commas and spaces, dropping empties and keeping the given order. */
export function parseTags(text: string): string[] {
  return text
    .split(/[\s,]+/)
    .map((tag) => tag.trim())
    .filter((tag) => tag !== '');
}

export interface DraftInput {
  number: string;
  name: string;
  type: string;
  parent: string;
  tags: string;
  currency: string;
}

/** Each error code is a message key in the catalogues (accounts.err.*). */
export interface DraftErrors {
  number?: 'required' | 'format' | 'taken';
  name?: 'required' | 'too_long';
  type?: 'required';
  parent?: 'same_type' | 'has_postings';
  tags?: 'format' | 'duplicate' | 'too_many';
  currency?: 'format';
}

/**
 * Validates the create form against the loaded chart. Uniqueness counts
 * archived accounts too: a retired number is never reused.
 */
export function validateDraft(draft: DraftInput, accounts: Account[]): DraftErrors {
  const errors: DraftErrors = {};
  const number = draft.number.trim();
  if (number === '') errors.number = 'required';
  else if (!NUMBER_PATTERN.test(number)) errors.number = 'format';
  else if (accounts.some((account) => account.number === number)) errors.number = 'taken';

  const name = draft.name.trim();
  if (name === '') errors.name = 'required';
  else if (name.length > MAX_NAME_LENGTH) errors.name = 'too_long';

  if (!(ACCOUNT_TYPES as readonly string[]).includes(draft.type)) errors.type = 'required';

  if (draft.parent !== '') {
    const parent = accounts.find((account) => account.number === draft.parent);
    if (parent && draft.type !== '' && parent.account_type !== draft.type) errors.parent = 'same_type';
    else if (parent && hasPostings(parent)) errors.parent = 'has_postings';
  }

  const tags = parseTags(draft.tags);
  if (tags.length > MAX_TAGS) errors.tags = 'too_many';
  else if (tags.some((tag) => !TAG_PATTERN.test(tag))) errors.tags = 'format';
  else if (new Set(tags).size !== tags.length) errors.tags = 'duplicate';

  const currency = draft.currency.trim();
  if (currency !== '' && !CURRENCY_PATTERN.test(currency)) errors.currency = 'format';

  return errors;
}

/** Whether any error is set. */
export function hasErrors(errors: DraftErrors): boolean {
  return Object.values(errors).some((value) => value !== undefined);
}

/** The accounts a new account may sit under: same class, not archived, no postings yet. */
export function parentOptions(accounts: Account[], type: string): Account[] {
  return accounts
    .filter((account) => account.account_type === type && !account.archived && !hasPostings(account))
    .sort(byNumberAsc);
}

/** Validates the tag list of an edit form (the same rule as the create form). */
export function tagErrors(text: string): DraftErrors['tags'] {
  const tags = parseTags(text);
  if (tags.length > MAX_TAGS) return 'too_many';
  if (tags.some((tag) => !TAG_PATTERN.test(tag))) return 'format';
  if (new Set(tags).size !== tags.length) return 'duplicate';
  return undefined;
}
