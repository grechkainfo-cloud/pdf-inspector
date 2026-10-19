# Page-boundary table controls

These PDFs contain synthetic travel-policy data. No corporate PDF, logo, barcode
payload or employee information is included. The barcode-shaped image is a
generated raster pattern, not an encoded identifier.

Regenerate with Python 3 (standard library only):

```console
python tests/fixtures/continued_tables/generate.py
cargo test --test table_continuation_tests
cargo test markdown::continuation
```

Commit the PDFs and expected results together with the generator. Expected cells
are defined in `generate.py`, independently of the extractor. `*.expected.md`
contains the complete logical table; `*.expected.json` also records row source
pages, context and the note. Normalize only whitespace around Markdown cell
delimiters and normalize LF/CRLF when comparing; changing, dropping or shifting
a value is a failure.

| PDF | Control |
| --- | --- |
| `one_page.pdf` | Ordinary six-column ruled table, no continuation |
| `single_row.pdf` | Last row alone on page 2, no header, notes below, raster barcode and folio in both footers |
| `barcode_caption.pdf` | Unique human-readable barcode identifiers do not close or enter the table; ordinary footer cleanup may remove them |
| `without_footer.pdf` | Same continuation with no footer; recovery must not depend on footer removal |
| `empty_values.pdf` | Zero, empty cell, dash and a nonzero amount remain different values |
| `repeated_header.pdf` | Repeated header on page 2 followed by records 4 and 5, then notes |
| `three_pages.pdf` | Ruled first page, borderless second page starting with a data row, lone final row on page 3 |
| `split_cell.pdf` | `Regional` / `managers` split across pages inside the last description; the next numbered record is retained |
| `independent_tables.pdf` | Same column count and positions, different header and restarted numbering; do not merge |
| `restarted_numbering.pdf` | Identical headers and positions, restarted numbering; these are independent tables |
| `closed_table.pdf` | Body text after the first table closes it; subsequent aligned text remains outside it |
| `no_table.pdf` | Prose must not become a table |

All twelve controls have been rendered and checked visually against the
generator's inputs. The common first page contains three six-column records;
the variants show the expected lone row, repeated/different header, empty cell,
zero, dash, split description, unique barcode caption or closing prose. The
middle page of `three_pages.pdf` was checked separately: records 4–37 are present
in order, followed by record 38 on page 3. This visual check does not replace
the automatic extraction tests.

The extraction tests run real PDF bytes through the public library API, without
an HTTP service or a model. They assert the complete expected table, not merely
the presence of individual amounts. They also check note preservation and
negative cases that must not be joined.

## Integration boundary

Full-document extraction can use the preceding page's table geometry. A request
that supplies only a continuation page cannot reconstruct a missing header from
an unavailable page. Clients should use full-document extraction when they need
one logical table across page boundaries.

Conversion of the restored Markdown table into independent prose blocks and
subsequent chunking belong to Pipex.Knowledge (mgerasim/Pipex#161). These controls
provide its extraction-side input/expected tables; its own unit and end-to-end
tests must cover that additional stage.
