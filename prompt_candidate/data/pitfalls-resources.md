Data work goes wrong in familiar ways. Avoid them:

- Downloading again on every run. Fetch once into a local cache and reuse it.
- Loading everything into memory. Stream or chunk large files, and pick column types up front.
- Quiet type guessing. Parse dates, time zones, numbers and missing markers explicitly.
- Mixing raw and derived files. Keep the originals untouched and write derived data to a separate place.
- Outputs too big to use. Aggregate to the size the next step actually needs.
