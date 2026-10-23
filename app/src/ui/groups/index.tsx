/** The group screens' routes: #/group, #/group/agreements (and new, edit, detail), #/group/reconciliation, #/group/view. */
import { t } from '../../i18n';
import { Empty, PageHead, Skeleton } from '../common';
import { AgreementDetail } from './AgreementDetail';
import { AgreementForm } from './AgreementForm';
import { AgreementsPage } from './Agreements';
import { GroupPage } from './GroupPage';
import { ReconciliationPage } from './Reconciliation';
import { GroupViewPage } from './GroupView';
import { GroupDirectory } from './GroupDirectory';
import { useGroupContext } from './context';

export function GroupRoute({ route }: { route: string }) {
  const rest = route === 'group' ? '' : route.slice('group/'.length);
  const ctx = useGroupContext();
  // A company in no group: the screens list the groups the person sees elsewhere.
  if (ctx.groups !== undefined && ctx.groups.length === 0) {
    // Until the groups elsewhere are read, do not say "no group" yet: they may be there.
    if (ctx.visibleGroups === undefined && ctx.error === undefined) return <Skeleton rows={3} label={t('view.loading')} />;
    if ((ctx.visibleGroups?.length ?? 0) > 0) {
      return <GroupDirectory groups={ctx.visibleGroups ?? []} companies={ctx.companies ?? []} pathname={window.location.pathname} />;
    }
  }
  if (rest === '') return <GroupPage />;
  if (rest === 'agreements') return <AgreementsPage />;
  if (rest === 'agreements/new') return <AgreementForm agreementId={null} />;
  if (rest === 'reconciliation') return <ReconciliationPage />;
  if (rest === 'view') return <GroupViewPage />;
  const edit = /^agreements\/([^/]+)\/edit$/.exec(rest);
  if (edit) return <AgreementForm agreementId={decodeURIComponent(edit[1] ?? '')} />;
  const detail = /^agreements\/([^/]+)$/.exec(rest);
  if (detail) return <AgreementDetail agreementId={decodeURIComponent(detail[1] ?? '')} />;
  return (
    <>
      <PageHead title={t('groups.title')} />
      <Empty title={t('shell.not_found')} />
    </>
  );
}
