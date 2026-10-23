/**
 * The shared avatar: a company's, group's or business unit's logo, or its
 * initials on a colour when it has no logo (or the logo cannot be shown). The
 * name is always written next to it by the caller, so the avatar is decorative.
 *
 * A logo is read through a signed link (documents/{id}/url) for the company
 * that holds it. Links are cached until shortly before they expire. Without
 * links on the server (503) the initials show instead.
 */
import { createContext, type ComponentChildren } from 'preact';
import { useContext, useEffect, useState } from 'preact/hooks';
import { absoluteUrl, documentUrl } from '../api/bills';
import type { ClientConfig } from '../api/client';
import { fallbackColour, initialsOf, textOn } from '../lib/logos';
import { useSession } from '../session';

/** Where a logo is: the document, and the company whose documents hold it (the page's company when absent). */
export interface LogoRef {
  documentId: string;
  companyId?: string;
}

export type AvatarSize = 'xs' | 'sm' | 'md' | 'lg';

/** The companies the person may see, for the switcher and the group screens to show their logos. */
export const CompaniesContext = createContext<{ id: string; name: string; logo_document_id?: string | null }[]>([]);

const REFRESH_MARGIN_MS = 60_000;
const links = new Map<string, { url: string; expires: number }>();
const pending = new Map<string, Promise<string>>();

/** The absolute link to a logo's file, cached until shortly before it expires. */
export function logoUrl(client: ClientConfig, logo: LogoRef): Promise<string> {
  const company = logo.companyId ?? client.company ?? '';
  const key = `${company}:${logo.documentId}`;
  const cached = links.get(key);
  if (cached && cached.expires - REFRESH_MARGIN_MS > Date.now()) return Promise.resolve(cached.url);
  const inFlight = pending.get(key);
  if (inFlight) return inFlight;
  const request = documentUrl({ ...client, company: logo.companyId ?? client.company }, logo.documentId)
    .then((link) => {
      const url = absoluteUrl(client, link.url);
      links.set(key, { url, expires: link.expires_at * 1000 });
      return url;
    })
    .finally(() => pending.delete(key));
  pending.set(key, request);
  return request;
}

/** The logo's URL, or null while it loads, when it has none, or when it cannot be read. */
function useLogoSource(logo: LogoRef | null | undefined): string | null {
  const client = useSession().client;
  const [src, setSrc] = useState<string | null>(null);
  const documentId = logo?.documentId ?? null;
  const companyId = logo?.companyId ?? null;
  useEffect(() => {
    if (!documentId) {
      setSrc(null);
      return;
    }
    let live = true;
    logoUrl(client, { documentId, companyId: companyId ?? undefined }).then(
      (url) => live && setSrc(url),
      () => live && setSrc(null),
    );
    return () => {
      live = false;
    };
  }, [client.baseUrl, documentId, companyId]);
  return src;
}

/** The avatar. `logo` null or absent shows initials on `colour` (or a colour from the name). */
export function Avatar({
  name,
  colour,
  logo,
  size = 'md',
  class: className,
}: {
  name: string;
  colour?: string | null;
  logo?: LogoRef | null;
  size?: AvatarSize;
  class?: string;
}) {
  const src = useLogoSource(logo);
  const [broken, setBroken] = useState(false);
  const bg = colour || fallbackColour(name);
  const classes = `avatar avatar-${size}${className ? ` ${className}` : ''}`;
  if (src && !broken) {
    return (
      <img class={`${classes} avatar-logo`} src={src} alt="" onError={() => setBroken(true)} />
    );
  }
  return (
    <span class={classes} style={{ background: bg, color: textOn(bg) }} aria-hidden="true" data-testid="avatar-initials">
      {initialsOf(name)}
    </span>
  );
}

/** A company's avatar: its logo from the companies the person may see, or its initials. */
export function CompanyAvatar({ companyId, name, size = 'md' }: { companyId: string; name: string; size?: AvatarSize }) {
  const companies = useContext(CompaniesContext);
  const company = companies.find((item) => item.id === companyId);
  const logo = company?.logo_document_id ? { documentId: company.logo_document_id, companyId } : null;
  return <Avatar name={name} logo={logo} size={size} />;
}

/** Wraps children with the companies the avatars look logos up in. */
export function CompaniesProvider({ companies, children }: { companies: { id: string; name: string; logo_document_id?: string | null }[]; children: ComponentChildren }) {
  return <CompaniesContext.Provider value={companies}>{children}</CompaniesContext.Provider>;
}
