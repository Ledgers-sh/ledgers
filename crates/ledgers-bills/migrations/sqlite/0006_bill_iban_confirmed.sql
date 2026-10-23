-- Changed-IBAN confirmation (issue #22): `iban_changed` keeps recording that a
-- bill's invoice IBAN differs from the vendor's, so an agent cannot pay it even
-- after a person has confirmed the bill. `iban_confirmed` records that a person
-- confirmed that IBAN; only an unconfirmed change blocks posting.

ALTER TABLE bill ADD COLUMN iban_confirmed INTEGER NOT NULL DEFAULT 0 CHECK (iban_confirmed IN (0, 1));
