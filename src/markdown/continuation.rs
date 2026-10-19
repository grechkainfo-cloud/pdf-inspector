//! Recover table rows at page boundaries while positioned text is still available.
//!
//! A single final row often fails all page-local table detectors. Reuse the previous
//! table's columns instead of trying to recover cell boundaries from flattened prose.

use std::collections::{HashMap, HashSet};

use crate::tables::{Table, TableKind};
use crate::types::TextItem;

use super::convert::PositionedMarkdown;

type PhysicalRow<'a> = Vec<(usize, &'a TextItem)>;
type PageImageRegions = HashMap<u32, Vec<(f32, f32, f32, f32)>>;

fn footer(item: &TextItem, items: &[TextItem], images: &PageImageRegions) -> bool {
    if item.y >= 55.0 {
        return false;
    }
    // A barcode may have a different human-readable identifier on every page.
    // Frequency alone cannot identify it. Treat text in/just below a small
    // bottom-margin image as footer evidence, without deleting it from output.
    if images.get(&item.page).is_some_and(|regions| {
        regions.iter().any(|&(x0, y0, x1, y1)| {
            let (left, right, bottom, top) = (x0.min(x1), x0.max(x1), y0.min(y1), y0.max(y1));
            bottom < 55.0
                && top - bottom <= 80.0
                && right - left >= 80.0
                && item.x >= left - 4.0
                && item.x + item.width <= right + 4.0
                && item.y >= bottom - 12.0
                && item.y <= top + 4.0
        })
    }) {
        return true;
    }
    let text = item.text.trim();
    if !text.is_empty()
        && text.chars().all(|c| c.is_ascii_digit())
        && (item.x >= 500.0 || item.x <= 40.0)
    {
        return true;
    }
    // Only repeated text in the bottom margin is ignorable. In particular, a
    // one-off note below the table closes it, even when printed in a small font.
    text.len() >= 10
        && items
            .iter()
            .filter(|other| {
                other.page != item.page
                    && (other.y - item.y).abs() <= 2.0
                    && other.text.trim() == text
            })
            .map(|other| other.page)
            .collect::<HashSet<_>>()
            .len()
            >= 2
}

fn ends_page(
    table: &Table,
    page: u32,
    items: &[TextItem],
    claimed: &HashSet<usize>,
    images: &PageImageRegions,
) -> bool {
    let Some(&bottom) = table.rows.last() else {
        return false;
    };
    let top = items
        .iter()
        .filter(|item| item.page == page)
        .map(|item| item.y)
        .fold(0.0_f32, f32::max);
    // Avoid treating a small table high on an otherwise blank page as open.
    bottom < top * 0.55
        && !items.iter().enumerate().any(|(index, item)| {
            item.page == page
                && item.y < bottom - item.height.max(3.0)
                && !claimed.contains(&index)
                && !footer(item, items, images)
        })
}

fn physical_rows<'a>(
    page: u32,
    items: &'a [TextItem],
    images: &PageImageRegions,
) -> Vec<PhysicalRow<'a>> {
    let mut sorted: Vec<_> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.page == page && !footer(item, items, images))
        .collect();
    sorted.sort_by(|(_, a), (_, b)| b.y.total_cmp(&a.y).then_with(|| a.x.total_cmp(&b.x)));
    let mut rows: Vec<PhysicalRow<'_>> = Vec::new();
    for pair in sorted {
        if let Some(row) = rows.last_mut() {
            if (row[0].1.y - pair.1.y).abs() <= 2.5 {
                row.push(pair);
                continue;
            }
        }
        rows.push(vec![pair]);
    }
    for row in &mut rows {
        row.sort_by(|(_, a), (_, b)| a.x.total_cmp(&b.x));
    }
    rows
}

fn column_ranges(table: &Table, page: u32, items: &[TextItem]) -> Vec<(f32, f32)> {
    let count = table.columns.len();
    let mut ranges = vec![(f32::INFINITY, f32::NEG_INFINITY); count];
    let top = table.rows.first().copied().unwrap_or(0.0) + 20.0;
    let bottom = table.rows.last().copied().unwrap_or(0.0) - 20.0;
    for item in items
        .iter()
        .filter(|item| item.page == page && item.y >= bottom && item.y <= top)
    {
        let text = item.text.trim();
        if text.is_empty() {
            continue;
        }
        let column = (0..count)
            .filter(|&col| {
                table
                    .cells
                    .iter()
                    .any(|row| row.get(col).is_some_and(|cell| cell.contains(text)))
            })
            .min_by(|&a, &b| {
                let center = item.x + item.width / 2.0;
                (table.columns[a] - center)
                    .abs()
                    .total_cmp(&(table.columns[b] - center).abs())
            });
        if let Some(col) = column {
            ranges[col].0 = ranges[col].0.min(item.x);
            ranges[col].1 = ranges[col].1.max(item.x + item.width);
        }
    }
    ranges
}

fn column_edges(table: &Table, page: u32, items: &[TextItem]) -> Vec<f32> {
    let count = table.columns.len();
    let ranges = column_ranges(table, page, items);
    let mut edges = vec![ranges[0].0.min(table.columns[0] - 15.0) - 5.0];
    for col in 1..count {
        let (left_end, right_start) = (ranges[col - 1].1, ranges[col].0);
        edges.push(
            if left_end.is_finite() && right_start.is_finite() && left_end < right_start {
                (left_end + right_start) / 2.0
            } else {
                (table.columns[col - 1] + table.columns[col]) / 2.0
            },
        );
    }
    edges.push(ranges[count - 1].1.max(table.columns[count - 1] + 25.0) + 5.0);
    edges
}

fn project(row: &PhysicalRow<'_>, table: &Table, edges: &[f32]) -> Option<Vec<String>> {
    let columns = &table.columns;
    if columns.len() < 2 || columns.windows(2).any(|pair| pair[1] <= pair[0]) {
        return None;
    }
    let mut cells = vec![String::new(); columns.len()];
    let last = columns.len() - 1;
    for (_, item) in row {
        if item.x < edges[0] || item.x + item.width > edges[last + 1] {
            return None;
        }
        let center = item.x + item.width / 2.0;
        let col = edges
            .windows(2)
            .position(|pair| center >= pair[0] && center < pair[1])
            .unwrap_or(last);
        // Prose spanning several columns is a boundary, never a table row.
        if item.x < edges[col] - 8.0 || item.x + item.width > edges[col + 1] + 8.0 {
            return None;
        }
        if !cells[col].is_empty() {
            cells[col].push(' ');
        }
        cells[col].push_str(item.text.trim());
    }
    Some(cells)
}

fn ordinal(text: &str) -> Option<u32> {
    text.trim_matches(|c: char| c.is_whitespace() || matches!(c, '*' | '.' | ')'))
        .parse()
        .ok()
}

fn numeric(text: &str) -> bool {
    !text.trim().is_empty()
        && text.chars().any(|c| c.is_ascii_digit())
        && text
            .chars()
            .all(|c| c.is_ascii_digit() || c.is_whitespace() || ",.-%()+−".contains(c))
}

fn last_record(table: &Table) -> Option<&Vec<String>> {
    table
        .cells
        .iter()
        .rev()
        .find(|row| row.first().is_some_and(|cell| !cell.is_empty()))
}

fn data_row(cells: &[String], table: &Table) -> bool {
    let Some(previous) = last_record(table) else {
        return false;
    };
    if cells.iter().filter(|cell| !cell.is_empty()).count() < cells.len().div_ceil(2).max(2) {
        return false;
    }
    if let Some(number) = previous.first().and_then(|cell| ordinal(cell)) {
        return cells.first().and_then(|cell| ordinal(cell)) == number.checked_add(1);
    }
    // Unnumbered continuation needs matching numeric columns, not just the
    // same column count. Text-only ambiguous continuations remain untouched.
    let numeric_columns: Vec<_> = previous
        .iter()
        .enumerate()
        .filter(|(_, cell)| numeric(cell))
        .map(|(col, _)| col)
        .collect();
    numeric_columns.len() >= 2
        && !cells[0].is_empty()
        && numeric_columns
            .iter()
            .all(|&col| cells.get(col).is_some_and(|cell| numeric(cell)))
}

fn wrapped_row(cells: &[String], table: &Table) -> bool {
    if !cells[0].is_empty() {
        return false;
    }
    let Some(previous) = table.cells.last() else {
        return false;
    };
    let mut found = false;
    for (cell, before) in cells.iter().zip(previous) {
        if cell.is_empty() {
            continue;
        }
        let lower = cell.to_lowercase();
        if ["note:", "notes:", "примечание:", "примечания:"]
            .iter()
            .any(|prefix| lower.starts_with(prefix))
        {
            return false;
        }
        // A lowercase unit fragment is not enough evidence for an unnumbered
        // table. Require explicit hyphenation there; numbered records provide
        // a stronger anchor for an ordinary wrapped description.
        if (previous.first().and_then(|cell| ordinal(cell)).is_none()
            && !before.ends_with('-'))
            || before.is_empty()
            || numeric(before)
            || numeric(cell)
            || !(before.ends_with('-') || cell.chars().next().is_some_and(char::is_lowercase))
        {
            return false;
        }
        found = true;
    }
    found
}

fn matching_geometry(first: &Table, next: &Table) -> bool {
    first.columns.len() == next.columns.len()
        && first
            .columns
            .iter()
            .zip(&next.columns)
            .all(|(a, b)| (a - b).abs() <= 8.0)
}

pub(super) fn repeated_header_rows(first: &Table, next: &Table) -> usize {
    first
        .cells
        .iter()
        .zip(&next.cells)
        .take_while(|(before, after)| {
            if before != after {
                return false;
            }
            let column_numbers = before
                .iter()
                .enumerate()
                .all(|(index, cell)| ordinal(cell) == Some(index as u32 + 1));
            column_numbers
                || (before.first().and_then(|cell| ordinal(cell)).is_none()
                    && before.iter().filter(|cell| numeric(cell)).count() < 2)
        })
        .count()
}

pub(super) fn numbered_records_continue(first: &Table, next: &Table, headers: usize) -> bool {
    let Some(previous) = last_record(first)
        .and_then(|row| row.first())
        .and_then(|cell| ordinal(cell))
    else {
        return true;
    };
    next.cells
        .iter()
        .skip(headers)
        .find_map(|row| row.first().and_then(|cell| ordinal(cell)))
        == previous.checked_add(1)
}

fn matching_anchors(
    first: &Table,
    first_page: u32,
    next: &Table,
    next_page: u32,
    items: &[TextItem],
) -> bool {
    if first.columns.len() != next.columns.len() {
        return false;
    }
    // Grid detectors store cell centres; heuristic detectors store text anchors.
    // Compare observed text positions when consecutive pages use different detectors.
    let before = column_ranges(first, first_page, items);
    let after = column_ranges(next, next_page, items);
    before
        .iter()
        .zip(after)
        .all(|(a, b)| a.0.is_finite() && b.0.is_finite() && (a.0 - b.0).abs() <= 12.0)
}

/// Extend the last table on a page only when there is no intervening body text
/// and the next page starts with a compatible table or aligned data row.
pub(super) fn recover_table_continuations(
    items: &[TextItem],
    claimed: &mut HashSet<usize>,
    tables: &mut HashMap<u32, Vec<PositionedMarkdown>>,
    images: &PageImageRegions,
) {
    let mut pages: Vec<_> = tables.keys().copied().collect();
    pages.sort_unstable();
    for origin in pages {
        let Some((index, source)) = tables.get(&origin).and_then(|blocks| {
            blocks
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| a.y.total_cmp(&b.y))
                .and_then(|(index, block)| block.table.clone().map(|table| (index, table)))
        }) else {
            continue;
        };
        if source.kind != TableKind::Data
            || source.columns.len() < 2
            || !tables[&origin][index].markdown.trim_end().ends_with('|')
        {
            continue;
        }
        let mut edges = column_edges(&source, origin, items);
        let mut combined = source.clone();
        let mut current = source;
        let mut page = origin;
        let mut changed = false;
        while ends_page(&current, page, items, claimed, images) {
            let next_page = page + 1;
            let rows = physical_rows(next_page, items, images);
            let Some(first_row) = rows.first() else { break };

            // A separately detected continuation may have a repeated header or
            // may have promoted its first data row to a Markdown header.
            let detected = tables.get(&next_page).and_then(|blocks| {
                blocks
                    .iter()
                    .enumerate()
                    .max_by(|(_, a), (_, b)| a.y.total_cmp(&b.y))
                    .and_then(|(idx, block)| block.table.as_ref().map(|table| (idx, table.clone())))
            });
            if let Some((next_index, next)) = detected {
                let at_top = next
                    .rows
                    .first()
                    .is_some_and(|&y| (y - first_row[0].1.y).abs() <= 20.0);
                let repeated = repeated_header_rows(&combined, &next);
                let continues = next
                    .cells
                    .first()
                    .is_some_and(|cells| data_row(cells, &current));
                log::debug!(
                    "table continuation {} -> {}: at_top={}, same_grid={}, same_anchors={}, repeated_headers={}, numbered_records={}, data_row={}, first_y={:?}, body_top={}",
                    page, next_page, at_top, matching_geometry(&current, &next),
                    matching_anchors(&current, page, &next, next_page, items), repeated,
                    numbered_records_continue(&current, &next, repeated), continues,
                    next.rows.first(), first_row[0].1.y
                );
                if at_top
                    && (matching_geometry(&current, &next)
                        || matching_anchors(&current, page, &next, next_page, items))
                    && ((repeated > 0 && numbered_records_continue(&current, &next, repeated))
                        || continues)
                {
                    combined
                        .cells
                        .extend(next.cells.iter().skip(repeated).cloned());
                    tables
                        .get_mut(&next_page)
                        .expect("detected page exists")
                        .remove(next_index);
                    edges = column_edges(&next, next_page, items);
                    current = next;
                    page = next_page;
                    changed = true;
                    continue;
                }
            }

            let mut used = Vec::new();
            let mut last_y = first_row[0].1.y;
            for row in &rows {
                if row.iter().any(|(idx, _)| claimed.contains(idx))
                    || last_y - row[0].1.y > row[0].1.height.max(10.0) * 2.5
                {
                    break;
                }
                let Some(cells) = project(row, &current, &edges) else {
                    break;
                };
                if !data_row(&cells, &current) && !wrapped_row(&cells, &current) {
                    break;
                }
                combined.cells.push(cells.clone());
                current.cells.push(cells);
                last_y = row[0].1.y;
                used.extend(row.iter().map(|(idx, _)| *idx));
            }
            if used.is_empty() {
                break;
            }
            claimed.extend(used);
            changed = true;
            current.rows = vec![first_row[0].1.y, last_y];
            page = next_page;
        }
        if changed {
            let block = &mut tables.get_mut(&origin).expect("origin exists")[index];
            block.markdown = crate::tables::table_to_markdown(&combined);
            block.table = Some(combined);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ItemType;

    fn item(text: &str, x: f32, y: f32, page: u32) -> TextItem {
        TextItem {
            text: text.into(),
            x,
            y,
            width: text.len() as f32 * 4.0,
            height: 9.0,
            font: "Helvetica".into(),
            font_size: 9.0,
            page,
            is_bold: false,
            is_italic: false,
            is_underline: false,
            is_strikeout: false,
            item_type: ItemType::Text,
            mcid: None,
        }
    }

    fn source() -> (
        Vec<TextItem>,
        HashSet<usize>,
        HashMap<u32, Vec<PositionedMarkdown>>,
    ) {
        let table = Table::new(
            vec![70., 180., 310., 370., 450., 540.],
            vec![300., 100.],
            vec![
                vec!["No", "Category", "City A", "City B", "Other", "Sites"]
                    .into_iter()
                    .map(String::from)
                    .collect(),
                vec!["3", "Staff", "15 680", "15 680", "8 230", "6 580"]
                    .into_iter()
                    .map(String::from)
                    .collect(),
            ],
            vec![],
        );
        let mut block =
            PositionedMarkdown::new(300., 70., crate::tables::table_to_markdown(&table), None);
        let mut items = vec![item("Travel policy", 55., 780., 1)];
        for (text, x) in [
            ("3", 65.),
            ("Staff", 95.),
            ("15 680", 298.),
            ("15 680", 358.),
            ("8 230", 440.),
            ("6 580", 530.),
        ] {
            items.push(item(text, x, 100., 1));
        }
        block.table = Some(table);
        (items, (1..=6).collect(), HashMap::from([(1, vec![block])]))
    }

    fn add_row(items: &mut Vec<TextItem>, number: &str, page: u32, y: f32) {
        for (text, x) in [
            (number, 65.),
            ("Other staff", 95.),
            ("10 450", 298.),
            ("10 450", 358.),
            ("6 910", 440.),
            ("4 180", 530.),
        ] {
            items.push(item(text, x, y, page));
        }
    }

    #[test]
    fn headerless_rows_use_the_latest_detected_table_boundaries() {
        let (mut items, mut claimed, mut tables) = source();
        let mut next = tables[&1][0].table.clone().unwrap();
        for column in &mut next.columns {
            *column += 8.0;
        }
        next.rows = vec![770.0, 100.0];
        next.cells.truncate(1);
        let first_index = items.len();
        for (text, x) in next.cells[0]
            .iter()
            .zip([65.0, 95.0, 298.0, 358.0, 440.0, 530.0])
        {
            items.push(item(text, x + 8.0, 770.0, 2));
        }
        for (number, y) in [("4", 740.0), ("5", 100.0)] {
            let start = items.len();
            add_row(&mut items, number, 2, y);
            let row = &mut items[start..];
            for value in row.iter_mut() {
                value.x += 8.0;
            }
            row[5].text = "1 234 567 890".into();
            row[5].width = 48.0;
            next.cells
                .push(row.iter().map(|value| value.text.clone()).collect());
        }
        claimed.extend(first_index..items.len());
        tables.insert(2, vec![super::super::positioned_table(&next, None)]);

        let start = items.len();
        add_row(&mut items, "6", 3, 770.0);
        for value in &mut items[start..] {
            value.x += 8.0;
        }
        items[start + 5].text = "1 234 567 890".into();
        items[start + 5].width = 48.0;

        recover_table_continuations(&items, &mut claimed, &mut tables, &HashMap::new());

        assert_eq!(
            tables[&1][0].table.as_ref().unwrap().cells[1..]
                .iter()
                .map(|row| row[0].as_str())
                .collect::<Vec<_>>(),
            vec!["3", "4", "5", "6"]
        );
        assert!(tables[&1][0]
            .markdown
            .contains("|6|Other staff|10 450|10 450|6 910|1 234 567 890|"));
        assert!(tables[&2].is_empty());
        assert!((start..items.len()).all(|index| claimed.contains(&index)));
    }

    #[test]
    fn line_grid_borders_are_normalized_to_cell_centres() {
        let table = Table::new(
            vec![55., 85., 285., 343., 405., 490., 570.],
            vec![805., 775.],
            vec![vec!["No", "Category", "City A", "City B", "Other", "Sites"]
                .into_iter()
                .map(String::from)
                .collect()],
            vec![],
        );
        let block = super::super::positioned_table(&table, None);
        assert_eq!(
            block.table.unwrap().columns,
            vec![70., 185., 314., 374., 447.5, 530.]
        );
    }

    #[test]
    fn lone_numbered_row_is_recovered_before_notes_and_footer() {
        let (mut items, mut claimed, mut tables) = source();
        add_row(&mut items, "4", 2, 770.);
        let note = items.len();
        items.push(item("note: receipts required", 95., 750., 2));
        items.push(item("2", 560., 35., 2));
        recover_table_continuations(&items, &mut claimed, &mut tables, &HashMap::new());
        assert!(tables[&1][0]
            .markdown
            .contains("|4|Other staff|10 450|10 450|6 910|4 180|"));
        assert!(!claimed.contains(&note));
        assert!(claimed.contains(&(note - 1)));
        assert!(!tables[&1][0].markdown.contains("receipts"));
    }

    #[test]
    fn unrelated_numbered_row_and_intervening_prose_are_not_consumed() {
        for with_prose in [false, true] {
            let (mut items, mut claimed, mut tables) = source();
            add_row(&mut items, if with_prose { "4" } else { "1" }, 2, 770.);
            if with_prose {
                items.push(item("New section", 55., 790., 2));
            }
            recover_table_continuations(&items, &mut claimed, &mut tables, &HashMap::new());
            assert_eq!(claimed, (1..=6).collect());
            assert!(!tables[&1][0].markdown.contains("Other staff"));
        }
    }

    #[test]
    fn large_vertical_gaps_do_not_absorb_later_records() {
        let (mut items, mut claimed, mut tables) = source();
        // Each continuation occupies the entire body of its page.
        add_row(&mut items, "4", 2, 350.);
        add_row(&mut items, "5", 2, 100.);
        // The large row gap intentionally ends the recovery: this must not
        // silently absorb arbitrary records farther down a page.
        add_row(&mut items, "6", 3, 770.);
        recover_table_continuations(&items, &mut claimed, &mut tables, &HashMap::new());
        assert!(tables[&1][0].markdown.contains("|4|Other staff|"));
        assert!(!tables[&1][0].markdown.contains("|5|Other staff|"));
        assert!(!tables[&1][0].markdown.contains("|6|Other staff|"));
    }

    #[test]
    fn text_after_source_table_closes_the_table() {
        let (mut items, mut claimed, mut tables) = source();
        items.push(item("Important conditions", 55., 75., 1));
        add_row(&mut items, "4", 2, 770.);
        recover_table_continuations(&items, &mut claimed, &mut tables, &HashMap::new());
        assert!(!tables[&1][0].markdown.contains("Other staff"));
    }

    #[test]
    fn column_counts_alone_do_not_make_a_data_row() {
        let (_, _, tables) = source();
        let table = tables[&1][0].table.as_ref().unwrap();
        let row: Vec<String> = ["No", "New category", "New city", "Amount", "Tax", "Total"]
            .into_iter()
            .map(String::from)
            .collect();
        assert!(!data_row(&row, table));
        assert!(!matching_geometry(
            table,
            &Table::new(vec![1., 2.], vec![], vec![], vec![])
        ));
    }

    #[test]
    fn numeric_values_and_empty_cells_are_not_invented() {
        assert!(numeric("10 450"));
        assert!(numeric("0"));
        assert!(!numeric(""));
        assert!(!numeric("—"));
        let (source_items, _, tables) = source();
        let table = tables[&1][0].table.as_ref().unwrap();
        let row = vec![
            (0, item("4", 65., 770., 2)),
            (1, item("Other staff", 95., 770., 2)),
            (2, item("0", 310., 770., 2)),
            (3, item("—", 450., 770., 2)),
        ];
        let borrowed: Vec<_> = row.iter().map(|(index, item)| (*index, item)).collect();
        let cells = project(&borrowed, table, &column_edges(table, 1, &source_items)).unwrap();
        assert_eq!(cells[2], "0");
        assert_eq!(cells[3], "");
        assert_eq!(cells[4], "—");
    }
}
