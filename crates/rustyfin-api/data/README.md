# Data files

Lists that `GET /Localization/Cultures` and `GET /Localization/Countries` return.

- `iso639-2.txt`: ISO 639-2 languages, one per line: `bibliographic|terminologic|alpha-2|English name`. From the
  Library of Congress list (https://www.loc.gov/standards/iso639-2/ISO-639-2_utf-8.txt, public domain), without its
  French names and the reserved `qaa-qtz` range.
- `iso3166-1.txt`: ISO 3166-1 countries, one per line: `alpha-2|alpha-3|English name` (the common name where
  there is one), sorted by name. From Debian's iso-codes 4.18.0 (`iso_3166-1.json`), LGPL-2.1-or-later.
