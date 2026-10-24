-- Widens connector_connection.connector to Spaceship (the registrar: domains and renewals) and Twilio (usage costs).
ALTER TABLE connector_connection DROP CONSTRAINT connector_connection_connector_check;
ALTER TABLE connector_connection ADD CONSTRAINT connector_connection_connector_check
    CHECK (connector IN ('cloudflare', 'anthropic', 'planetscale', 'aws', 'vercel', 'gcp', 'azure', 'supabase', 'openai', 'openrouter', 'stripe', 'polar', 'spaceship', 'twilio'));
