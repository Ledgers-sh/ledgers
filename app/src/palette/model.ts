/**
 * The command palette's model: every operation in the server's OpenAPI
 * document, with the three forms the server publishes for it (the CLI line,
 * the MCP tool name and the HTTP call), and what this person may do with it.
 *
 * Nothing here is hand-written per command. The list comes from the document
 * the server serves; the only app knowledge is which screen shows a command's
 * result (SCREEN_FOR), and a command without one is labelled as API and CLI only.
 */
import type { MessageKey } from '../i18n';

export interface PaletteCommand {
  /** The command name, e.g. "entries.post". */
  command: string;
  /** The operationId, e.g. "entries_post". */
  operationId: string;
  method: string;
  path: string;
  tier: string;
  humanOnly: boolean;
  /** `ledgers entries post`, or null when the server did not publish one. */
  cli: string | null;
  /** The MCP tool name, or null for a human-only command. */
  mcp: string | null;
  /** The curl form of the HTTP call, or null when the server did not publish one. */
  curl: string | null;
}

/** Roles that may run every tier. Any other company role may only read (the people sign-in admits owners and admins). */
export const FULL_ACCESS_ROLES: readonly string[] = ['owner', 'admin'];

/** The signed-in person (roles) or an agent key (its tier is applied by the server). */
export type Session = { kind: 'person'; roles: readonly string[] } | { kind: 'key' };

const TIER_RANK: Record<string, number> = { read: 0, draft: 1, 'post-within-limits': 2 };

/** Where a command's result shows in this app. Commands not listed here are API and CLI only. */
export const SCREEN_FOR: Record<string, { hash: string; label: MessageKey }> = {
  'entries.post': { hash: '#/new', label: 'shell.new_entry' },
  'entries.list': { hash: '#/journal', label: 'nav.journal' },
  'entries.get': { hash: '#/journal', label: 'nav.journal' },
  'entries.reverse': { hash: '#/journal', label: 'nav.journal' },
  'accounts.list': { hash: '#/accounts', label: 'nav.accounts' },
  'reports.trial_balance': { hash: '#/reports', label: 'nav.reports' },
  'reports.profit_and_loss': { hash: '#/reports', label: 'nav.reports' },
  'reports.balance_sheet': { hash: '#/reports', label: 'nav.reports' },
  'reports.account_balance_by_dimension': { hash: '#/reports', label: 'nav.reports' },
  'bills.match': { hash: '#/bills', label: 'nav.bills' },
};

const METHODS = ['get', 'post', 'put', 'patch', 'delete'];

function str(value: unknown): string | null {
  return typeof value === 'string' && value !== '' ? value : null;
}

/** Every operation of an OpenAPI 3 document as a palette command, sorted by command name. */
export function paletteCommands(document: unknown): PaletteCommand[] {
  const paths = document && typeof document === 'object' ? (document as { paths?: unknown }).paths : undefined;
  if (!paths || typeof paths !== 'object') return [];
  const commands: PaletteCommand[] = [];
  for (const [path, item] of Object.entries(paths)) {
    if (!item || typeof item !== 'object') continue;
    for (const [method, operation] of Object.entries(item)) {
      if (!METHODS.includes(method) || !operation || typeof operation !== 'object') continue;
      const op = operation as Record<string, unknown>;
      const command = str(op['x-ledgers-command']);
      const operationId = str(op.operationId);
      if (!command || !operationId) continue;
      commands.push({
        command,
        operationId,
        method: method.toUpperCase(),
        path,
        tier: str(op['x-ledgers-tier']) ?? 'read',
        humanOnly: op['x-ledgers-human-only'] === true,
        cli: str(op['x-ledgers-cli']),
        mcp: str(op['x-ledgers-mcp']),
        curl: str(op['x-ledgers-curl']),
      });
    }
  }
  return commands.sort((a, b) => a.command.localeCompare(b.command));
}

/** What this session may do with a command, and the reason when it may not. */
export type Access =
  | { runnable: true; hash: string; label: MessageKey }
  | { runnable: false; reason: 'human-only' | 'role' | 'no-screen' };

/**
 * Whether the palette offers a command in this app. A command runs here only
 * when it has a screen and the session may run its tier. Agent keys cannot
 * run human-only commands; people can, from their screen.
 */
export function accessFor(command: PaletteCommand, session: Session): Access {
  if (session.kind === 'key' && command.humanOnly) return { runnable: false, reason: 'human-only' };
  if (session.kind === 'person' && !session.roles.some((role) => FULL_ACCESS_ROLES.includes(role))) {
    if ((TIER_RANK[command.tier] ?? 2) > TIER_RANK.read) return { runnable: false, reason: 'role' };
  }
  const screen = SCREEN_FOR[command.command];
  if (!screen) return { runnable: false, reason: command.humanOnly ? 'human-only' : 'no-screen' };
  return { runnable: true, hash: screen.hash, label: screen.label };
}

/** Filters commands by any of their names or forms. An empty query keeps everything. */
export function filterCommands(commands: readonly PaletteCommand[], query: string): PaletteCommand[] {
  const needle = query.trim().toLowerCase();
  if (needle === '') return [...commands];
  return commands.filter((command) =>
    [command.command, command.operationId, command.cli, command.mcp, command.method, command.path, command.tier]
      .some((field) => field !== null && field.toLowerCase().includes(needle)),
  );
}
