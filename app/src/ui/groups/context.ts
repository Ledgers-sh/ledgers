/**
 * The data every group screen starts from: the companies the person belongs
 * to (with their roles), the groups they may see, and the company the page is
 * in with the person's role there. Roles decide what a screen offers; the
 * server decides again on every command.
 */
import { companiesList, type CompanySummary } from '../../api/companies';
import { groupsList, type GroupView } from '../../api/groups';
import type { ClientConfig } from '../../api/client';
import { useAsync } from '../../hooks';
import { companyFromSearch } from '../../lib/companies';
import { useSession } from '../../session';

export interface GroupContext {
  client: ClientConfig;
  companies: CompanySummary[] | undefined;
  groups: GroupView[] | undefined;
  /** The first error of the two reads, if any. */
  error: unknown;
  reload: () => void;
  /** The company the page is in. */
  currentCompany: string;
  /** The person's role in the company the page is in. */
  role: string;
  /** The company's name, or its id when the person cannot see its name. */
  nameOf: (companyId: string) => string;
}

export function useGroupContext(): GroupContext {
  const session = useSession();
  const client = session.client;
  const person = session.person;
  const companies = useAsync(() => companiesList(client), [client.baseUrl]);
  const groups = useAsync(() => groupsList(client), [client.baseUrl]);
  const currentCompany = companyFromSearch(window.location.search) ?? person?.companyId ?? '';
  const role = companies.data?.find((company) => company.id === currentCompany)?.role ?? person?.role ?? '';
  const list = companies.data ?? [];
  return {
    client,
    companies: companies.data,
    groups: groups.data,
    error: companies.error ?? groups.error,
    reload: () => {
      companies.reload();
      groups.reload();
    },
    currentCompany,
    role,
    nameOf: (companyId: string) => list.find((company) => company.id === companyId)?.name ?? companyId,
  };
}
