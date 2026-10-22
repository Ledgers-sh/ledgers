/**
 * Approval requests for the signed-in person (issue #33). Agents get 403 on
 * every call here; the app only offers this screen to a person session.
 * Shapes follow the approvals API contract; unknown extra fields are ignored
 * and optional fields may be absent.
 */
import { request, type ClientConfig } from './client';
import type { Side } from './types';

export type ApprovalStatus = 'pending' | 'approved' | 'rejected' | 'expired' | 'superseded';
export type ApprovalRisk = 'low' | 'high';

/** One line of the entry an approval would post. `amount` is the core's canonical amount string. */
export interface ApprovalPreviewLine {
  account: string;
  side: Side;
  amount: string;
  currency: string;
}

export interface ApprovalPreview {
  lines: ApprovalPreviewLine[];
  /** Totals per currency, as canonical amount strings. */
  totals?: Record<string, string>;
}

export interface ApprovalView {
  id: string;
  status: ApprovalStatus;
  command: string;
  risk: ApprovalRisk;
  /** Currency code to canonical amount string, e.g. { "EUR": "12500" }; {} when there is no amount. */
  amount: Record<string, string>;
  counterparty?: string | null;
  reason: string;
  /** The requesting agent's key id. */
  requested_by: string;
  created_at: string;
  expires_at: string;
  decided_at?: string | null;
  decided_by?: string | null;
  decision_reason?: string | null;
  /** Command output after an approval, e.g. the posted entry's reference. */
  result?: Record<string, unknown> | null;
  /** Audit event id of the decision. */
  audit_ref?: string | null;
  request_hash: string;
  /** Present on GET by id; null or absent in lists. */
  preview?: ApprovalPreview | null;
}

export interface ApprovalList {
  pending_count: number;
  items: ApprovalView[];
}

export interface ApprovalDecisionOutput {
  approval: ApprovalView;
  result?: unknown;
}

export interface BulkDecisionResult {
  id: string;
  approval: ApprovalView;
  result?: unknown;
}

export interface BulkDecisionOutput {
  results: BulkDecisionResult[];
}

/** GET /v1/approval-requests. "pending" or "decided" (approved, rejected, expired, superseded), newest first. */
export function approvalsList(config: ClientConfig, status: 'pending' | 'decided', limit = 50): Promise<ApprovalList> {
  return request<ApprovalList>(config, {
    method: 'GET',
    path: '/v1/approval-requests',
    query: { status, limit: String(limit) },
  });
}

/** GET /v1/approval-requests/{id}, with the entry preview. */
export function approvalGet(config: ClientConfig, id: string): Promise<ApprovalView> {
  return request<ApprovalView>(config, {
    method: 'GET',
    path: `/v1/approval-requests/${encodeURIComponent(id)}`,
  });
}

/**
 * POST /v1/approval-requests/{id}/decision. A reject may carry a reason; an
 * approve carries none. A high-risk approve may answer 401 reauth-required.
 */
export function approvalDecide(
  config: ClientConfig,
  id: string,
  input: { decision: 'approve' } | { decision: 'reject'; reason?: string },
): Promise<ApprovalDecisionOutput> {
  return request<ApprovalDecisionOutput>(config, {
    method: 'POST',
    path: `/v1/approval-requests/${encodeURIComponent(id)}/decision`,
    body: input,
  });
}

/**
 * POST /v1/approval-requests/bulk-decision. Approves low-risk requests in one
 * currency. The server refuses the whole call (422 bulk-selection-refused)
 * before writing anything when a selected request is high-risk or currencies differ.
 */
export function approvalsBulkApprove(config: ClientConfig, ids: string[]): Promise<BulkDecisionOutput> {
  return request<BulkDecisionOutput>(config, {
    method: 'POST',
    path: '/v1/approval-requests/bulk-decision',
    body: { ids, decision: 'approve' },
  });
}
