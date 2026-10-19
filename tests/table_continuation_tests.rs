//! Control PDFs generated from known data; no external services or private documents.

use pdf_inspector::{process_pdf_mem_with_options, MarkdownOptions, PdfOptions};

fn extract(pdf: &[u8], strip_footers: bool) -> String {
    process_pdf_mem_with_options(
        pdf,
        PdfOptions::new().markdown(MarkdownOptions {
            strip_headers_footers: strip_footers,
            ..Default::default()
        }),
    )
    .expect("control PDF should extract")
    .markdown
    .expect("text PDF should produce Markdown")
}

fn check_table(actual: &str, expected: &str) {
    // include_str! retains the fixture's Windows CRLF, while Markdown output
    // uses LF. Compare lines, without weakening any cell-value assertions.
    let expected = expected.lines().collect::<Vec<_>>().join("\n");
    // Normalize only insignificant spaces around Markdown cells. A missing,
    // duplicated or shifted cell remains a failure.
    let normalized = actual
        .lines()
        .map(|line| {
            if line.starts_with('|') {
                line.split('|').map(str::trim).collect::<Vec<_>>().join("|")
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        normalized.contains(expected.trim()),
        "expected table:\n{expected}\nactual:\n{actual}"
    );
    for row in expected.lines().skip(2) {
        assert_eq!(
            normalized.matches(row).count(),
            1,
            "row must appear exactly once: {row}\n{actual}"
        );
    }
}

fn table_blocks(markdown: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut block = Vec::new();
    for line in markdown.lines().chain(std::iter::once("")) {
        if line.starts_with('|') {
            block.push(line.split('|').map(str::trim).collect::<Vec<_>>().join("|"));
        } else if !block.is_empty() {
            blocks.push(block.join("\n"));
            block.clear();
        }
    }
    blocks
}

#[test]
fn lone_continuation_row_with_barcode_footer_and_notes() {
    for strip_footers in [false, true] {
        let md = extract(
            include_bytes!("fixtures/continued_tables/single_row.pdf"),
            strip_footers,
        );
        check_table(
            &md,
            include_str!("fixtures/continued_tables/single_row.expected.md"),
        );
        assert!(md.contains("Note: receipts are required."), "{md}");
        assert!(md.contains("Travel policy"), "{md}");
        assert!(
            !md.contains("[Image:"),
            "barcode must not enter the table: {md}"
        );
        assert!(md.find("|4|Other staff|").unwrap() < md.find("Note:").unwrap());
    }
}

#[test]
fn unique_barcode_captions_do_not_close_or_enter_the_table() {
    let md = extract(
        include_bytes!("fixtures/continued_tables/barcode_caption.pdf"),
        true,
    );
    check_table(
        &md,
        include_str!("fixtures/continued_tables/barcode_caption.expected.md"),
    );
    let items = pdf_inspector::extract_text_with_positions_mem(include_bytes!(
        "fixtures/continued_tables/barcode_caption.pdf"
    ))
    .unwrap();
    assert!(items.iter().any(|item| item.text == "DOC-0001"));
    assert!(items.iter().any(|item| item.text == "DOC-0002"));
    assert!(
        !table_blocks(&md).iter().any(|block| block.contains("DOC-")),
        "{md}"
    );
}

#[test]
fn ordinary_table_is_unchanged() {
    let md = extract(
        include_bytes!("fixtures/continued_tables/one_page.pdf"),
        true,
    );
    check_table(
        &md,
        include_str!("fixtures/continued_tables/one_page.expected.md"),
    );
}

#[test]
fn empty_values_zero_and_dash_remain_distinct() {
    let md = extract(
        include_bytes!("fixtures/continued_tables/empty_values.pdf"),
        true,
    );
    check_table(
        &md,
        include_str!("fixtures/continued_tables/empty_values.expected.md"),
    );
}

#[test]
fn continuation_does_not_depend_on_a_footer() {
    let md = extract(
        include_bytes!("fixtures/continued_tables/without_footer.pdf"),
        true,
    );
    check_table(
        &md,
        include_str!("fixtures/continued_tables/without_footer.expected.md"),
    );
}

#[test]
fn continuation_crosses_three_pages_without_losing_a_headerless_first_row() {
    let md = extract(
        include_bytes!("fixtures/continued_tables/three_pages.pdf"),
        true,
    );
    check_table(
        &md,
        include_str!("fixtures/continued_tables/three_pages.expected.md"),
    );
    assert!(md.contains("Note: receipts are required."), "{md}");
}

#[test]
fn repeated_header_is_removed_without_losing_the_first_data_row() {
    let md = extract(
        include_bytes!("fixtures/continued_tables/repeated_header.pdf"),
        true,
    );
    check_table(
        &md,
        include_str!("fixtures/continued_tables/repeated_header.expected.md"),
    );
    assert_eq!(md.matches("|No|Category|").count(), 1, "{md}");
    assert!(md.contains("Note: receipts are required."), "{md}");
}

#[test]
fn split_description_cell_is_joined_and_next_record_preserved() {
    let md = extract(
        include_bytes!("fixtures/continued_tables/split_cell.pdf"),
        true,
    );
    check_table(
        &md,
        include_str!("fixtures/continued_tables/split_cell.expected.md"),
    );
    assert!(md.contains("Note: receipts are required."), "{md}");
}

#[test]
fn independent_tables_with_equal_column_counts_are_not_merged() {
    let md = extract(
        include_bytes!("fixtures/continued_tables/independent_tables.pdf"),
        true,
    );
    assert!(md.contains("|No|Category|"), "{md}");
    assert!(md.contains("|No|Tax class|"), "{md}");
    assert_eq!(
        table_blocks(&md),
        table_blocks(include_str!(
            "fixtures/continued_tables/independent_tables.expected.md"
        )),
        "{md}"
    );
    assert_eq!(
        md.lines().filter(|line| line.starts_with("|---|")).count(),
        2,
        "{md}"
    );
}

#[test]
fn identical_headers_with_restarted_numbering_are_separate_tables() {
    let md = extract(
        include_bytes!("fixtures/continued_tables/restarted_numbering.pdf"),
        true,
    );
    assert_eq!(md.matches("|No|Category|").count(), 2, "{md}");
    assert_eq!(
        table_blocks(&md),
        table_blocks(include_str!(
            "fixtures/continued_tables/restarted_numbering.expected.md"
        )),
        "{md}"
    );
    assert_eq!(
        md.lines().filter(|line| line.starts_with("|---|")).count(),
        2,
        "{md}"
    );
}

#[test]
fn body_text_after_a_table_prevents_continuation() {
    let md = extract(
        include_bytes!("fixtures/continued_tables/closed_table.pdf"),
        true,
    );
    assert!(md.contains("This table ends here."), "{md}");
    assert_eq!(
        table_blocks(&md),
        table_blocks(include_str!(
            "fixtures/continued_tables/closed_table.expected.md"
        )),
        "{md}"
    );
    assert!(
        !md.contains("|4|Other staff|"),
        "closed table must not absorb later prose: {md}"
    );
    assert!(
        md.contains("Other staff"),
        "unclaimed text must remain available: {md}"
    );
}

#[test]
fn ordinary_document_keeps_its_text() {
    let md = extract(
        include_bytes!("fixtures/continued_tables/no_table.pdf"),
        true,
    );
    assert!(md.contains("Ordinary document without tables."), "{md}");
    assert!(
        md.contains("Amounts stay in prose: 10 450 per day."),
        "{md}"
    );
    assert!(!md.contains("|---|"), "{md}");
}

#[test]
fn unnumbered_property_table_does_not_absorb_unit_fragments() {
    let md = extract(include_bytes!("fixtures/thermo-freon12.pdf"), true);
    let expected = include_str!("snapshots/thermo-freon12.md")
        .lines()
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(md.trim_end(), expected.trim_end());
}
