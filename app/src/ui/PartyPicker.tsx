/**
 * A party chosen from the server's list by typing: a combobox with the usual
 * keys (arrow keys move, Enter chooses, Escape closes). It can create a party
 * on the spot when the typed name is new, so a bill or a merge never waits on
 * a separate screen.
 */
import { useEffect, useId, useRef, useState } from 'preact/hooks';
import { partiesList, partyCreate, type Party, type PartyViesResult } from '../api/parties';
import { ApiError } from '../api/client';
import { t } from '../i18n';
import { Ltr } from '../i18n/Ltr';
import { useSession } from '../session';
import { VatChip } from './PartyParts';
import { vatStatus } from '../lib/parties';

const MAX_OPTIONS = 8;

/** What the picker shows for the chosen party: a bill's vendor may be known only by id, name and VAT ID. */
export interface PickedParty {
  id: string;
  name: string;
  vat_id: string | null;
  /** Left out when the VAT check is not known for this party (a bill's vendor). */
  latest_vies?: PartyViesResult | null;
}

export function PartyPicker({
  id,
  label,
  selected,
  onPick,
  exclude,
  allowCreate = false,
  disabled,
  describedBy,
}: {
  id: string;
  label: string;
  selected: PickedParty | null;
  onPick: (party: Party) => void;
  /** A party that may not be chosen (the party being merged, for example). */
  exclude?: string;
  /** Offer to create a party from the typed name when nothing matches. */
  allowCreate?: boolean;
  disabled?: boolean;
  describedBy?: string;
}) {
  const { client, toast } = useSession();
  const listId = useId();
  const [text, setText] = useState('');
  const [open, setOpen] = useState(false);
  const [options, setOptions] = useState<Party[]>([]);
  const [active, setActive] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const seq = useRef(0);
  // The latest client, read by the search below without making every new client object a new search.
  const clientRef = useRef(client);
  clientRef.current = client;

  useEffect(() => {
    if (!open) return;
    const term = text.trim();
    const mine = ++seq.current;
    const timer = setTimeout(() => {
      partiesList(clientRef.current, { q: term === '' ? undefined : term, limit: MAX_OPTIONS })
        .then((page) => {
          if (mine !== seq.current) return;
          setOptions(page.parties.filter((p) => p.id !== exclude && !p.merged_into));
          setActive(0);
        })
        .catch(() => mine === seq.current && setOptions([]));
    }, 180);
    return () => clearTimeout(timer);
  }, [text, open, exclude]);

  const exact = options.some((p) => p.name.trim().toLowerCase() === text.trim().toLowerCase());
  const createRow = allowCreate && text.trim() !== '' && !exact;
  const rowCount = options.length + (createRow ? 1 : 0);

  const choose = (party: Party) => {
    onPick(party);
    setText('');
    setOpen(false);
    setError(null);
  };

  const create = async () => {
    setBusy(true);
    setError(null);
    try {
      const made = await partyCreate(client, { name: text.trim(), roles: 'vendor' });
      toast(t('parties.picker_created'), 'ok');
      choose(made.party);
    } catch (err) {
      setError(err instanceof ApiError && err.detail ? err.detail : t('parties.picker_create_failed'));
    } finally {
      setBusy(false);
    }
  };

  const onKey = (event: KeyboardEvent) => {
    if (event.key === 'ArrowDown') {
      event.preventDefault();
      setOpen(true);
      setActive((a) => (rowCount === 0 ? 0 : (a + 1) % rowCount));
    } else if (event.key === 'ArrowUp') {
      event.preventDefault();
      setActive((a) => (rowCount === 0 ? 0 : (a - 1 + rowCount) % rowCount));
    } else if (event.key === 'Enter' && open && rowCount > 0) {
      event.preventDefault();
      if (active < options.length) choose(options[active]);
      else void create();
    } else if (event.key === 'Escape') {
      setOpen(false);
    }
  };

  const activeId = open && rowCount > 0 ? `${listId}-opt-${active}` : undefined;

  return (
    <div class="party-picker">
      <label for={id} class="party-picker-label">
        {label}
      </label>
      {selected && !open && (
        <div class="party-picker-current">
          <span class="party-picker-name">
            <Ltr>{selected.name}</Ltr>
          </span>
          {selected.latest_vies !== undefined && <VatChip status={vatStatus({ vat_id: selected.vat_id, latest_vies: selected.latest_vies })} />}
          <button type="button" class="btn btn-sm btn-ghost" disabled={disabled} onClick={() => setOpen(true)}>
            {t('parties.picker_change')}
          </button>
        </div>
      )}
      <input
          id={id}
          class="input"
          type="text"
          hidden={selected !== null && !open}
          role="combobox"
          autoComplete="off"
          disabled={disabled}
          aria-autocomplete="list"
          aria-expanded={open && rowCount > 0 ? 'true' : 'false'}
          aria-controls={listId}
          aria-activedescendant={activeId}
          aria-describedby={describedBy}
          placeholder={t('parties.picker_placeholder')}
          value={text}
          onFocus={() => setOpen(true)}
          onInput={(event) => {
            // Results for the text before this one are not a choice any more: Enter must not take them.
            setText((event.target as HTMLInputElement).value);
            setOptions([]);
            setActive(0);
            setOpen(true);
          }}
          onKeyDown={onKey}
          onBlur={() => setTimeout(() => setOpen(false), 150)}
        />
      {open && (
        <ul id={listId} role="listbox" class="party-picker-list" aria-label={label}>
          {options.map((party, index) => (
            <li
              key={party.id}
              id={`${listId}-opt-${index}`}
              role="option"
              aria-selected={index === active ? 'true' : 'false'}
              class={`party-picker-option${index === active ? ' active' : ''}`}
              onMouseDown={(event) => event.preventDefault()}
              onClick={() => choose(party)}
            >
              <span>
                <Ltr>{party.name}</Ltr>
                {party.vat_id && (
                  <span class="muted party-picker-vat">
                    {' '}
                    <Ltr>{party.vat_id}</Ltr>
                  </span>
                )}
              </span>
              <VatChip status={vatStatus(party)} />
            </li>
          ))}
          {createRow && (
            <li
              key="create"
              id={`${listId}-opt-${options.length}`}
              role="option"
              aria-selected={active === options.length ? 'true' : 'false'}
              class={`party-picker-option create${active === options.length ? ' active' : ''}`}
              aria-busy={busy ? 'true' : undefined}
              onMouseDown={(event) => event.preventDefault()}
              onClick={() => {
                if (!busy) void create();
              }}
            >
              {t('parties.picker_create', { name: text.trim() })}
            </li>
          )}
          {options.length === 0 && !createRow && <li class="party-picker-empty muted">{t('parties.picker_none')}</li>}
        </ul>
      )}
      {error && (
        <span class="field-error" role="alert">
          {error}
        </span>
      )}
    </div>
  );
}
