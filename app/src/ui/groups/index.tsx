/** The group screens' routes: #/group, #/group/agreements (and new, edit, detail), #/group/reconciliation, #/group/view. */
import { t } from '../../i18n';
import { Empty, PageHead } from '../common';
import { AgreementDetail } from './AgreementDetail';
import { AgreementForm } from './AgreementForm';
import { AgreementsPage } from './Agreements';
import { GroupPage } from './GroupPage';
import { ReconciliationPage } from './Reconciliation';
import { GroupViewPage } from './GroupView';

export function GroupRoute({ route }: { route: string }) {
  const rest = route === 'group' ? '' : route.slice('group/'.length);
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
