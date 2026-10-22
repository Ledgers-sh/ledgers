import { describe, expect, it } from 'vitest';
import { readerKey } from './BillReview';
import { en } from '../i18n/en';

describe('readerKey', () => {
  it('names each reader the review screen can show', () => {
    expect(readerKey('rules')).toBe('bills.reader_rules');
    expect(readerKey('minimax')).toBe('bills.reader_minimax');
    expect(readerKey('anthropic')).toBe('bills.reader_anthropic');
    expect(readerKey('unread-scan')).toBe('bills.reader_none');
  });

  it('treats a hand-typed bill and an unknown reader as typed by hand', () => {
    expect(readerKey(null)).toBe('bills.reader_manual');
    expect(readerKey('something-new')).toBe('bills.reader_manual');
  });

  it('has English text for every key it can return', () => {
    for (const reader of ['rules', 'minimax', 'anthropic', 'unread-scan', null]) {
      expect(en[readerKey(reader)]).toBeTruthy();
    }
    expect(en['bills.needs_photo']).toContain('photo');
  });
});
