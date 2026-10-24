-- Widens connector_connection.connector to the cloud connectors (gcp, azure, supabase).
ALTER TABLE connector_connection DROP CONSTRAINT connector_connection_connector_check;
ALTER TABLE connector_connection ADD CONSTRAINT connector_connection_connector_check
    CHECK (connector IN ('cloudflare', 'anthropic', 'planetscale', 'aws', 'vercel', 'gcp', 'azure', 'supabase'));
