/**
 * Connected sources (issue #142): the connections, their sync runs, the
 * allocation rules and the costs by unit. Paths and shapes come from
 * crates/ledgers-connect (commands.rs) and the registry.
 *
 * A token is write-only: it is sent once, on create or update, and no response
 * carries it. The app never keeps one after the request.
 */
import { request, type ClientConfig } from './client';

export interface CredentialField {
  name: string;
  label: string;
}

export interface ConnectorView {
  id: string;
  name: string;
  credential_fields: CredentialField[];
  hosts: string[];
  config_keys: string[];
}

export interface ConnectionView {
  id: string;
  connector: string;
  connector_name: string;
  name: string;
  config: Record<string, unknown>;
  default_unit: string;
  is_demo: boolean;
  has_credential: boolean;
  last_sync_at: string | null;
  created_at: string;
  updated_at: string;
}

export interface ConnectionsListOutput {
  connections: ConnectionView[];
  connectors: ConnectorView[];
}

export interface RunView {
  id: string;
  window_since: string;
  window_until: string;
  status: 'succeeded' | 'failed' | string;
  invoices_found: number;
  bills_created: number;
  usage_rows: number;
  error_message: string | null;
  notes: string[];
  finished_at: string;
}

export interface AllocationRule {
  pattern: string;
  unit_code: string;
}

export interface UnmappedResource {
  resource_kind: string;
  resource_id: string;
  resource_name: string;
  periods: string[];
  suggested_unit: string | null;
}

export interface ConnectionAllocation {
  connection_id: string;
  name: string;
  default_unit: string;
  rules: AllocationRule[];
  unmapped: UnmappedResource[];
}

export interface UnitChoice {
  code: string;
  name: string;
}

export interface AllocationListOutput {
  connections: ConnectionAllocation[];
  units: UnitChoice[];
}

export interface AllocationSetOutput {
  rules: AllocationRule[];
  resplit: number;
  kept_frozen: number;
}

export interface CostLine {
  period: string;
  unit_code: string;
  connector: string;
  connection_id: string;
  currency: string;
  amount_minor: number;
}

export interface CostsOutput {
  lines: CostLine[];
}

export interface CreateConnectionInput {
  connector: string;
  name: string;
  config: Record<string, unknown>;
  default_unit: string;
  /** The token, sent once, for a connector with one token field. Never returned. */
  token?: string;
  /** The credential fields, sent once, for a connector with several. Never returned. */
  credential?: Record<string, string>;
}

/** The answer of `connections.test`: nothing is written. `error` carries no credential. */
export interface TestConnectionOutput {
  ok: boolean;
  notes: string[];
  error: string | null;
}

export interface UpdateConnectionInput {
  name?: string;
  config?: Record<string, unknown>;
  default_unit?: string;
  /** A new token, when rotating it. Omit to keep the stored one. */
  token?: string;
}

/** GET /v1/connections (connections.list): the connections and the connectors Ledgers ships. */
export function connectionsList(config: ClientConfig): Promise<ConnectionsListOutput> {
  return request<ConnectionsListOutput>(config, { method: 'GET', path: '/v1/connections' });
}

/** POST /v1/connections (connections.create). A person only (owner or admin). */
export function connectionsCreate(config: ClientConfig, input: CreateConnectionInput): Promise<ConnectionView> {
  return request<ConnectionView>(config, { method: 'POST', path: '/v1/connections', body: input });
}

/** POST /v1/connections/{id}/update (connections.update). A person only. */
export function connectionsUpdate(config: ClientConfig, id: string, input: UpdateConnectionInput): Promise<ConnectionView> {
  return request<ConnectionView>(config, {
    method: 'POST',
    path: `/v1/connections/${encodeURIComponent(id)}/update`,
    body: input,
  });
}

/** POST /v1/connections/{id}/delete (connections.delete): removes the token; drafted bills stay. */
export function connectionsDelete(config: ClientConfig, id: string): Promise<{ id: string; disconnected: boolean }> {
  return request(config, { method: 'POST', path: `/v1/connections/${encodeURIComponent(id)}/delete`, body: {} });
}

/** POST /v1/connections/{id}/sync (connections.sync_now): reads the vendor now. A person only. */
/** POST /v1/connections/{id}/test (connections.test). Reads the vendor for today; writes nothing. A person only. */
export function connectionsTest(config: ClientConfig, id: string): Promise<TestConnectionOutput> {
  return request<TestConnectionOutput>(config, {
    method: 'POST',
    path: `/v1/connections/${encodeURIComponent(id)}/test`,
    body: {},
  });
}

/** One domain due for renewal, as the registrar lists it. Metadata only: no price. */
export interface RenewalDomain {
  name: string;
  expires_on: string;
  auto_renew: boolean;
  status: string | null;
}

export interface RenewalsOutput {
  ok: boolean;
  error: string | null;
  as_of: string;
  window_days: number;
  domains: RenewalDomain[];
  notes: string[];
}

/** POST /v1/connections/{id}/renewals (connections.renewals). Reads the registrar's domains; writes nothing. A person only. */
export function connectionRenewals(config: ClientConfig, id: string): Promise<RenewalsOutput> {
  return request<RenewalsOutput>(config, {
    method: 'POST',
    path: `/v1/connections/${encodeURIComponent(id)}/renewals`,
    body: {},
  });
}

export function connectionsSyncNow(config: ClientConfig, id: string): Promise<RunView> {
  return request<RunView>(config, {
    method: 'POST',
    path: `/v1/connections/${encodeURIComponent(id)}/sync`,
    body: {},
  });
}

/** GET /v1/connections/{id}/runs (connections.runs): the sync history, newest first. */
export function connectionRuns(config: ClientConfig, id: string, limit = 20): Promise<{ runs: RunView[] }> {
  return request(config, {
    method: 'GET',
    path: `/v1/connections/${encodeURIComponent(id)}/runs`,
    query: { limit: String(limit) },
  });
}

/** GET /v1/allocation-rules (allocation_rules.list): rules, default units and the needs-mapping queue. */
export function allocationRulesList(config: ClientConfig, connectionId?: string): Promise<AllocationListOutput> {
  return request<AllocationListOutput>(config, {
    method: 'GET',
    path: '/v1/allocation-rules',
    query: connectionId ? { connection_id: connectionId } : undefined,
  });
}

/** POST /v1/allocation-rules/{connection_id} (allocation_rules.set): replaces the rules and re-splits unposted drafts. */
export function allocationRulesSet(
  config: ClientConfig,
  connectionId: string,
  rules: AllocationRule[],
  defaultUnit?: string,
): Promise<AllocationSetOutput> {
  return request<AllocationSetOutput>(config, {
    method: 'POST',
    path: `/v1/allocation-rules/${encodeURIComponent(connectionId)}`,
    body: { rules, ...(defaultUnit ? { default_unit: defaultUnit } : {}) },
  });
}

/** GET /v1/reports/costs-by-unit (reports.costs_by_unit): connected costs per unit, source and month. */
export function reportsCostsByUnit(config: ClientConfig, from: string, to: string): Promise<CostsOutput> {
  return request<CostsOutput>(config, { method: 'GET', path: '/v1/reports/costs-by-unit', query: { from, to } });
}
