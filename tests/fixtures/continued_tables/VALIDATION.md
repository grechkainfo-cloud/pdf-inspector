# Validation of page-boundary table recovery

Validated on Windows before the final conservative guard was added:

- 826 library unit tests passed.
- All 12 synthetic PDF extraction controls passed, including repeated headers,
  three pages, unique barcode captions, empty values and independent tables.
- All twelve generated PDFs were rendered and visually reviewed.
- The private travel-policy PDF was extracted again: record 4 occurs once with
  six cells and amounts 10 450, 10 450, 6 910 and 4 180; notes follow the table.
  The private PDF is not included in this repository.
- The existing 2013-app2 snapshot was updated after verifying that every data
  cell and row order remained identical. Only spurious page-boundary Markdown
  headers were removed. Its snapshot test passed.
- Existing integration tests: 143 passed, 9 failed after that snapshot update.
  Five snapshot failures differ only by CRLF/LF. Three failures occur while
  parsing existing synthetic PDF inputs, before Markdown recovery. Freon showed
  a genuine false continuation of an isolated unit fragment.

The Freon comparison against the unchanged checkout identified an overly broad
wrapped-cell heuristic. The final guard requires explicit hyphenation for
unnumbered wrapped cells; numbered descriptions still support ordinary wraps.
A thirteenth public-API regression test compares the full Freon output against
its existing snapshot, normalizing CRLF/LF only.

**The final guard and thirteenth regression test have not been executed:** the
F: drive holding the temporary Rust toolchain and build outputs became unavailable.
Re-run `cargo test --lib --test table_continuation_tests` before merging.

Subsequent review also fixed the legacy merge chain to compare record numbering
against the last accepted page and refreshed column boundaries after accepting
a detected continuation. Two unit tests cover a three-page merge with a
restarted-numbering negative case and a headerless row following a shifted,
wider detected table. These changes have not been compiled or executed either:
the current rustup installation has no installed toolchain. The final working
tree passed `git diff --check`; the checks above describe earlier revisions.

Other checks:

- Changed Rust files passed rustfmt before that final guard/test addition.
- Repository-wide fmt reports existing differences in HTTP source/tests.
- Strict clippy on Rust 1.99 reports existing chunks_exact_to_as_chunks and
  manual_slice_fill lints. Clippy with only those two categories allowed passed.
- The server-feature test run was not validated: the Windows GNU DLL export
  limit prevented a normal build, and a temporary external linker workaround
  produced an unusable test executable. No linker workaround is committed.
- The sibling pdf-evals checkout is unavailable; its snapshot/semantic evaluation
  was not run.

Development deployment has not been updated. Table-to-prose conversion before
chunking remains a separate Pipex.Knowledge change in mgerasim/Pipex#161.
