"""Generate small, non-confidential PDF regression fixtures using only Python stdlib.

Run: python tests/fixtures/continued_tables/generate.py
The expected rows below are input data, never harvested from extractor output.
"""

from pathlib import Path
import json

ROOT = Path(__file__).resolve().parent
EDGES = [55, 85, 285, 343, 405, 490, 570]
TEXT_X = [65, 95, 300, 360, 440, 530]
HEADER = ["No", "Category", "City A", "City B", "Other", "Sites"]
ROWS = [
    ["1", "Directors", "41 810", "41 810", "20 900", "13 170"],
    ["2", "Deputies", "26 130", "26 130", "13 170", "9 870"],
    ["3", "Managers", "15 680", "15 680", "8 230", "6 580"],
    ["4", "Other staff", "10 450", "10 450", "6 910", "4 180"],
]


def text(value, x, y, size=9):
    value = value.replace("\\", "\\\\").replace("(", "\\(").replace(")", "\\)")
    return f"BT /F1 {size} Tf 1 0 0 1 {x} {y} Tm ({value}) Tj ET\n"


def row(values, y):
    return "".join(text(value, x, y) for value, x in zip(values, TEXT_X) if value)


def grid(top, count, height=70):
    bottom = top - count * height
    ops = "0.5 w\n"
    for x in EDGES:
        ops += f"{x} {bottom} m {x} {top} l S\n"
    for index in range(count + 1):
        y = top - index * height
        ops += f"{EDGES[0]} {y} m {EDGES[-1]} {y} l S\n"
    return ops


def footer(page, caption=False):
    return ("q 140 0 0 20 415 18 cm /Barcode Do Q\n" + text(str(page), 560, 42)
            + (text(f"DOC-{page:04}", 440, 8, 7) if caption else ""))


def first_page():
    return (
        text("Travel policy - lodging limits, currency units per day", 55, 780, 12)
        + text("The following limits apply to staff categories.", 55, 750)
        + grid(360, 4)
        + row(HEADER, 330)
        + "".join(row(values, y) for values, y in zip(ROWS[:3], [260, 190, 120]))
        + footer(1)
    )


def write_pdf(path, pages):
    objects = [b"<< /Type /Catalog /Pages 2 0 R >>", b"", b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"]
    # A real raster image, not path strokes that could themselves look like a table.
    barcode = bytes(0 if (x // 2) % 3 else 255 for _y in range(20) for x in range(128))
    objects.append(b"<< /Type /XObject /Subtype /Image /Width 128 /Height 20 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length "
                   + str(len(barcode)).encode() + b" >>\nstream\n" + barcode + b"\nendstream")
    page_ids = []
    for content in pages:
        page_id = len(objects) + 1
        page_ids.append(page_id)
        stream_id = page_id + 1
        objects.append(f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 3 0 R >> /XObject << /Barcode 4 0 R >> >> /Contents {stream_id} 0 R >>".encode())
        data = content.encode("ascii")
        objects.append(f"<< /Length {len(data)} >>\nstream\n".encode() + data + b"endstream")
    objects[1] = ("<< /Type /Pages /Kids [" + " ".join(f"{p} 0 R" for p in page_ids) + f"] /Count {len(page_ids)} >>").encode()
    pdf = bytearray(b"%PDF-1.4\n")
    offsets = [0]
    for number, body in enumerate(objects, 1):
        offsets.append(len(pdf))
        pdf.extend(f"{number} 0 obj\n".encode() + body + b"\nendobj\n")
    xref = len(pdf)
    pdf.extend(f"xref\n0 {len(offsets)}\n0000000000 65535 f \n".encode())
    for offset in offsets[1:]:
        pdf.extend(f"{offset:010} 00000 n \n".encode())
    pdf.extend(f"trailer\n<< /Size {len(offsets)} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode())
    path.write_bytes(pdf)


def expected(name, rows, pages):
    lines = ["|" + "|".join(HEADER) + "|", "|" + "---|" * len(HEADER)]
    lines.extend("|" + "|".join(values) + "|" for values in rows)
    (ROOT / f"{name}.expected.md").write_text("\n".join(lines) + "\n", encoding="utf-8")
    (ROOT / f"{name}.expected.json").write_text(json.dumps({
        "headers": HEADER, "rows": rows, "source_pages": pages,
        "context": "Travel policy - lodging limits, currency units per day",
        "note": "Note: receipts are required.",
    }, indent=2) + "\n", encoding="utf-8")


def negative_expected(name, tables, prose):
    blocks = []
    for header, rows, page in tables:
        blocks.append("\n".join(["|" + "|".join(header) + "|", "|" + "---|" * len(header)]
                                + ["|" + "|".join(values) + "|" for values in rows]))
    (ROOT / f"{name}.expected.md").write_text("\n\n".join(blocks) + "\n", encoding="utf-8")
    (ROOT / f"{name}.expected.json").write_text(json.dumps({
        "tables": [{"headers": header, "rows": rows, "source_page": page} for header, rows, page in tables],
        "preserved_prose": prose,
    }, indent=2) + "\n", encoding="utf-8")


def main():
    ROOT.mkdir(exist_ok=True)
    note = text("Note: receipts are required.", 55, 725)
    write_pdf(ROOT / "single_row.pdf", [first_page(), row(ROWS[3], 770) + note + footer(2)])
    expected("single_row", ROWS, [1, 1, 1, 2])
    write_pdf(ROOT / "barcode_caption.pdf", [first_page().replace(footer(1), footer(1, True)),
              row(ROWS[3], 770) + note + footer(2, True)])
    expected("barcode_caption", ROWS, [1, 1, 1, 2])

    write_pdf(ROOT / "one_page.pdf", [first_page()])
    expected("one_page", ROWS[:3], [1, 1, 1])
    write_pdf(ROOT / "without_footer.pdf", [
        first_page().replace(footer(1), ""), row(ROWS[3], 770) + note,
    ])
    expected("without_footer", ROWS, [1, 1, 1, 2])

    missing = ["4", "Other staff", "0", "", "-", "4 180"]
    write_pdf(ROOT / "empty_values.pdf", [first_page(), row(missing, 770) + note + footer(2)])
    expected("empty_values", ROWS[:3] + [missing], [1, 1, 1, 2])

    middle_rows = [[str(n), f"Staff group {n}", "10 450", "10 450", "6 910", "4 180"] for n in range(4, 38)]
    middle = "".join(row(values, 770 - offset * 20) for offset, values in enumerate(middle_rows)) + footer(2)
    final = ["38", "Last group", "10 450", "10 450", "6 910", "4 180"]
    write_pdf(ROOT / "three_pages.pdf", [first_page(), middle, row(final, 770) + note + footer(3)])
    expected("three_pages", ROWS[:3] + middle_rows + [final], [1, 1, 1] + [2] * len(middle_rows) + [3])

    second = grid(805, 3, 30) + row(HEADER, 785) + row(ROWS[3], 755)
    fifth = ["5", "Temporary staff", "0", "0", "0", "0"]
    second += row(fifth, 725) + text("Note: receipts are required.", 55, 680) + footer(2)
    write_pdf(ROOT / "repeated_header.pdf", [first_page(), second])
    expected("repeated_header", ROWS + [fifth], [1, 1, 1, 2, 2])

    different_header = ["No", "Tax class", "Rate A", "Rate B", "Rate C", "Rate D"]
    independent = grid(805, 4, 50) + row(different_header, 785)
    independent += "".join(row(values, y) for values, y in zip(ROWS[:3], [735, 685, 635]))
    write_pdf(ROOT / "independent_tables.pdf", [first_page(), independent + footer(2)])
    negative_expected("independent_tables", [(HEADER, ROWS[:3], 1), (different_header, ROWS[:3], 2)], [])
    restarted = independent
    for before, after in zip(different_header, HEADER):
        restarted = restarted.replace(f"({before})", f"({after})")
    write_pdf(ROOT / "restarted_numbering.pdf", [first_page(), restarted + footer(2)])
    negative_expected("restarted_numbering", [(HEADER, ROWS[:3], 1), (HEADER, ROWS[:3], 2)], [])

    closed = first_page().replace(footer(1), text("This table ends here.", 55, 65) + footer(1))
    write_pdf(ROOT / "closed_table.pdf", [closed, row(ROWS[3], 770) + note + footer(2)])
    negative_expected("closed_table", [(HEADER, ROWS[:3], 1)], ["This table ends here.", "Other staff", "Note: receipts are required."])

    no_table = text("Ordinary document without tables.", 55, 780) + text("Amounts stay in prose: 10 450 per day.", 55, 750)
    no_table += "".join(text(f"Paragraph {n}: receipts and policy conditions apply.", 55, 720 - n * 25) for n in range(1, 9)) + footer(1)
    write_pdf(ROOT / "no_table.pdf", [no_table])
    negative_expected("no_table", [], ["Ordinary document without tables.", "Amounts stay in prose: 10 450 per day."])

    # A lower-case fragment continues the last description, not a new record.
    split = first_page().replace("(Managers)", "(Regional)")
    fragment = row(["", "managers", "", "", "", ""], 770)
    write_pdf(ROOT / "split_cell.pdf", [split, fragment + row(ROWS[3], 750) + note + footer(2)])
    split_rows = [values[:] for values in ROWS]
    split_rows[2][1] = "Regional managers"
    expected("split_cell", split_rows, [1, 1, [1, 2], 2])


if __name__ == "__main__":
    main()
