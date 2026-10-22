-- When the person last authenticated at the issuer (ID token auth_time), epoch seconds.
ALTER TABLE people_session ADD COLUMN auth_time INTEGER;
