/**
 * The server's problem slugs for the party screens, as message keys. The
 * server names each refusal with a slug (`party-vat-invalid`, `party-bank-
 * needs-person`, ...); the app shows the message in the reader's language.
 * A slug the app has no message for returns null, and the caller shows the
 * server's own words instead.
 */
import { ApiError } from '../api/client';
import { en, type MessageKey } from '../i18n/en';

/** The message key of a party problem, or null when the error is not one the app words. */
export function partyProblemKey(error: unknown): MessageKey | null {
  if (!(error instanceof ApiError) || !error.slug) return null;
  const key = `problem.${error.slug}`;
  return key in en ? (key as MessageKey) : null;
}

/** The message key of a problem slug named in a CSV row report, or null. */
export function slugMessageKey(slug: string): MessageKey | null {
  const key = `problem.${slug}`;
  return key in en ? (key as MessageKey) : null;
}
