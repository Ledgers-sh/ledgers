-- The group's logo. The group row is shared by its members, so the logo is one
-- stored document: logo_document_id names it, and logo_company_id names the
-- company whose documents hold it. A member reads the logo with that company
-- as the request's company. Both are NULL when the group has no logo.
ALTER TABLE company_group ADD COLUMN logo_document_id TEXT;
ALTER TABLE company_group ADD COLUMN logo_company_id TEXT;
