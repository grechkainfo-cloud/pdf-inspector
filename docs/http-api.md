# HTTP API

Сервер `pdf-api` открывает по HTTP всю функциональность библиотеки
`pdf-inspector`: классификацию PDF, извлечение текста и Markdown, работу с
областями страницы и восстановление таблиц.

Запуск и упаковка описаны в [`docs/deployment.md`](deployment.md).
Машиночитаемое описание доступно на `GET /openapi.json`.

```bash
cargo run --release --features server --bin pdf-api
# или
docker run -d -p 8080:8080 pdf-inspector:latest
```

---

## Быстрый старт

```bash
# PDF → Markdown
curl -F file=@document.pdf http://localhost:8080/v1/process

# Только Markdown, без JSON-обёртки
curl -F file=@document.pdf 'http://localhost:8080/v1/process?format=markdown'

# Быстрая классификация: нужен ли OCR
curl -F file=@document.pdf http://localhost:8080/v1/classify

# Сырое тело вместо multipart
curl --data-binary @document.pdf \
     -H 'Content-Type: application/pdf' \
     'http://localhost:8080/v1/process?pages=1-5&compact=true'
```

---

## Как передавать PDF

Любой эндпоint принимает документ в одном из трёх видов — выбирайте тот,
который проще вашему клиенту.

### 1. `multipart/form-data`

Файл в поле `file`, параметры — либо отдельными текстовыми полями, либо одним
полем `options` с JSON-объектом.

```bash
curl -F file=@document.pdf \
     -F pages=1-3 \
     -F compact=true \
     http://localhost:8080/v1/process

curl -F file=@document.pdf \
     -F 'options={"pages": [1, 2], "password": "secret"}' \
     http://localhost:8080/v1/process
```

### 2. `application/json`

Файл в base64 в поле `pdf_base64`, параметры — соседними ключами. Допустим и
`data:`-префикс (`data:application/pdf;base64,...`).

```bash
jq -n --arg pdf "$(base64 -w0 document.pdf)" \
   '{pdf_base64: $pdf, pages: [1,2], compact: true}' \
| curl -H 'Content-Type: application/json' -d @- \
       http://localhost:8080/v1/process
```

Эндпоинты с составными параметрами (`/v1/regions/*`, `/v1/tables/*`) удобнее
всего вызывать именно так.

### 3. Сырое тело

Тело запроса — сам PDF, параметры — в query string.

```bash
curl --data-binary @document.pdf \
     -H 'Content-Type: application/pdf' \
     'http://localhost:8080/v1/text?format=text&password=secret'
```

Правила приоритета: параметры из тела перекрывают одноимённые из query string.
Неизвестные ключи игнорируются. Значения в query string и в текстовых полях
формы разбираются как JSON, если это возможно (`compact=true` → булево,
`pages=[1,2]` → массив), кроме `password`, `mode`, `profile`, `format`,
`strategy` — они всегда остаются строками.

---

## Аутентификация

Если сервер запущен с `PDF_INSPECTOR_API_KEY`, все `/v1/*` требуют ключ:

```bash
curl -H "Authorization: Bearer $KEY" ...
curl -H "X-API-Key: $KEY" ...
```

`/`, `/health`, `/version` и `/openapi.json` открыты всегда — иначе
healthcheck и балансировщик не смогут опрашивать сервис.

---

## Эндпоинты

### Служебные

| Метод | Путь | Назначение |
|---|---|---|
| GET | `/` | Список эндпоинтов |
| GET | `/health`, `/healthz`, `/readyz` | Проба живости |
| GET | `/version` | Версия сборки |
| GET | `/openapi.json` | Описание OpenAPI 3.1 |

### Документ целиком

#### `POST /v1/process` (синоним `/v1/markdown`)

Полный конвейер: определение типа → извлечение → Markdown.

```json
{
  "pdf_type": "text_based",
  "page_count": 12,
  "processing_time_ms": 143,
  "has_text": true,
  "markdown_length": 18422,
  "markdown": "# Отчёт\n\n...",
  "pages_needing_ocr": [7],
  "ocr_reasons_by_page": [{ "page": 7, "reasons": ["scanned"] }],
  "title": "Годовой отчёт",
  "confidence": 0.95,
  "layout": {
    "is_complex": true,
    "pages_with_tables": [3, 4],
    "pages_with_columns": [2]
  },
  "has_encoding_issues": false
}
```

`pages_needing_ocr` и `layout.*` — номера страниц с единицы. Коды в
`ocr_reasons_by_page`: `scanned`, `no_text`, `vector_text`,
`suspected_garbled_text`.

При `format=markdown` возвращается сам Markdown с типом `text/markdown`.

#### `POST /v1/detect`

Только определение типа: читает метаданные и выборку страниц, текст не
извлекает — самый быстрый вызов по документу. `markdown` всегда `null`.

#### `POST /v1/analyze`

Определение типа + извлечение + анализ сложности вёрстки, без генерации
Markdown. Ответ той же формы, что у `/v1/process`.

#### `POST /v1/classify`

Облегчённая классификация для маршрутизации (10–50 мс). Параметров не
принимает. **Номера страниц с нуля.**

```json
{
  "pdf_type": "mixed",
  "page_count": 12,
  "pages_needing_ocr": [6],
  "confidence": 0.8
}
```

#### `POST /v1/text`

Плоский текст без структурирования. `format=text` отдаёт `text/plain`.

```json
{ "text": "...", "length": 15320 }
```

#### `POST /v1/items`

Позиционированные фрагменты текста со шрифтами и начертанием — основа для
собственной вёрстки или отладки.

```json
{
  "total_items": 842,
  "underlined_count": 3,
  "items": [
    {
      "text": "Итого",
      "page": 1,
      "x": 72.0,
      "y": 690.5,
      "width": 34.2,
      "height": 11.0,
      "font": "Helvetica-Bold",
      "font_size": 11.0,
      "is_bold": true,
      "is_italic": false,
      "is_underline": false,
      "is_strikeout": false,
      "item_type": "text",
      "mcid": 12
    }
  ]
}
```

`y` — в системе координат PDF (начало отсчёта внизу слева). У элементов
`item_type: "link"` добавляется поле `url`.

#### `POST /v1/pages/markdown`

Markdown отдельно по каждой странице плюс метаданные вёрстки — для гибридных
конвейеров, где простые страницы берутся напрямую, а сложные уходят в OCR.
Статистика шрифтов считается по всему документу, поэтому пороги заголовков не
зависят от выбора страниц.

**`pages` здесь считается с нуля** (соответствует библиотечному вызову).

```json
{
  "pages": [
    { "page": 0, "markdown": "# Заголовок\n...", "needs_ocr": false, "ocr_reason": null }
  ],
  "pages_with_tables": [3],
  "pages_with_columns": [],
  "pages_needing_ocr": [7],
  "ocr_reasons_by_page": [{ "page": 7, "reasons": ["scanned"] }],
  "is_complex": true
}
```

### Области страницы

Координаты — в пунктах PDF с началом отсчёта **вверху слева** (как отдаёт
модель разметки после пересчёта координат).

#### `POST /v1/regions/text`

Текст внутри заданных прямоугольников. Каждая область получает флаг
`needs_ocr` — он поднимается, когда текста нет, шрифты GID-кодированы или
извлечённое содержимое не проходит проверку на «мусор».

```json
{
  "pdf_base64": "JVBERi0xLjQK...",
  "page_regions": [
    { "page": 0, "regions": [[72, 96, 540, 320], [72, 340, 540, 700]] }
  ]
}
```

```json
{
  "pages": [
    {
      "page": 0,
      "regions": [
        { "text": "...", "needs_ocr": false, "ocr_reason": null },
        { "text": "", "needs_ocr": true, "ocr_reason": "suspected_garbled_text" }
      ]
    }
  ]
}
```

Результаты идут параллельно входным областям. Каждый фрагмент текста
попадает ровно в одну область — ту, с которой перекрытие больше, — поэтому
пересекающиеся области не дублируют содержимое.

#### `POST /v1/regions/tables`

Тот же вход, но внутри каждой области запускается распознавание таблиц и
возвращается Markdown-таблица. Если таблица не найдена, `text` пуст и
`needs_ocr` равен `true`.

#### `POST /v1/regions/vector-grid`

Ищет сетку из линий или прямоугольников внутри одной области. Ответ по форме
совпадает со входом эндпоинтов `/v1/tables/structure*`, поэтому таблицу с
нарисованной разлиновкой можно обработать вообще без TSR-модели.

```json
{ "pdf_base64": "...", "page": 0, "region": [72, 96, 540, 320], "render_dpi": 200 }
```

```json
{
  "detected": true,
  "structure_tokens": ["<table>", "<tr>", "<td></td>", "..."],
  "cell_bboxes": [[10, 12, 240, 48]]
}
```

### Таблицы по внешней структуре (TSR)

Структуру таблицы даёт внешняя модель (например, SLANet), запущенная на
отрендеренном фрагменте страницы; текст ячеек pdf-inspector берёт из самого
PDF — OCR не используется.

Общий вход для трёх эндпоинтов:

```json
{
  "pdf_base64": "...",
  "tables": [
    {
      "page": 0,
      "crop_pdf_pt_bbox": [72, 96, 540, 320],
      "render_dpi": 200,
      "structure_tokens": ["<table>", "<tr>", "<td></td>", "<td></td>", "</tr>", "</table>"],
      "cell_bboxes": [[0, 0, 640, 60], [640, 0, 1300, 60]]
    }
  ]
}
```

`cell_bboxes` — в пикселях фрагмента, по одному на каждый открывающий тег
ячейки, в порядке следования; допускаются 4 значения или полигон из 8.

| Эндпоинт | Результат |
|---|---|
| `POST /v1/tables/structure` | Markdown как есть, без контроля качества |
| `POST /v1/tables/structure/cells` | Ячейки с координатами, объединениями и флагом заголовка |
| `POST /v1/tables/structure/auto` | Markdown с автоисправлением — **рекомендуется** |

`/auto` распознаёт типичные сбои TSR (фантомные пустые строки, несколько
строк, слипшихся в одну ячейку), чинит их на месте или заменяет вывод
эвристическим разбором, а в `fallback_reason` сообщает, какой путь сработал
(`null` — сработал основной).

```json
{
  "tables": [
    { "markdown": "| A | B |\n| --- | --- |\n| 1 | 2 |", "fallback_reason": null },
    { "markdown": "...", "fallback_reason": "multi_row_in_cell_expanded" }
  ]
}
```

Ответ `/v1/tables/structure/cells`:

```json
{
  "tables": [
    {
      "cells": [
        {
          "row": 0, "col": 0, "rowspan": 1, "colspan": 2,
          "is_header": true, "text": "Период",
          "page_pt_bbox": [72.0, 96.0, 300.0, 118.0]
        }
      ]
    }
  ]
}
```

---

## Параметры

### `/v1/process`, `/v1/detect`, `/v1/analyze`

| Параметр | Тип | По умолчанию | Описание |
|---|---|---|---|
| `pages` | список / число / строка | все | Страницы **с единицы**: `[1,2]`, `3`, `"1,3,5-10"` |
| `password` | строка | — | Пароль зашифрованного PDF |
| `mode` | `detect` \| `analyze` \| `full` | по эндпоинту | Глубина обработки |
| `format` | `json` \| `markdown` | `json` | Отдать Markdown напрямую |
| `profile` | `fidelity` \| `compact` | `fidelity` | `compact` схлопывает многоточия и прочее «дорогое» форматирование |
| `compact` | булево | `false` | То же, что `profile=compact` |
| `include_page_numbers` | булево | `false` | Вставлять маркеры `<!-- Page N -->` |
| `detect_headers` | булево | `true` | Заголовки по размеру шрифта |
| `detect_lists` | булево | `true` | Списки |
| `detect_code` | булево | `true` | Блоки кода |
| `detect_bold` / `detect_italic` / `detect_underline` | булево | `true` | Начертания |
| `base_font_size` | число | авто | Базовый кегль для сравнения |
| `remove_page_numbers` | булево | `true` | Убирать колонцифры |
| `format_urls` | булево | `true` | Оформлять ссылки |
| `fix_hyphenation` | булево | `true` | Склеивать перенесённые слова |
| `include_images` | булево | `false` | Плейсхолдеры изображений |
| `include_links` | булево | `true` | Извлекать гиперссылки |
| `strip_headers_footers` | булево | `true` | Убирать повторяющиеся колонтитулы |
| `strategy` | `early_exit` \| `full` \| `sample` \| `pages` | `sample` | Какие страницы смотрит детектор |
| `sample_size` | число | `8` | Размер выборки для `strategy=sample` |
| `detect_pages` | список | — | Страницы для `strategy=pages` |
| `min_text_ops_per_page` | число | `3` | Порог текстовых операторов |
| `text_page_ratio_threshold` | число | `0.6` | Доля текстовых страниц для классификации |

### `/v1/text`, `/v1/items`

`pages` (с единицы), `password`, а для `/v1/text` ещё `format=text`.

### `/v1/pages/markdown`

`pages` — **с нуля**. Порядок ответа повторяет порядок запроса.

---

## Ошибки

Все ошибки — один формат:

```json
{ "error": { "code": "encrypted", "message": "PDF is encrypted — supply the correct `password`" } }
```

| HTTP | `code` | Когда |
|---|---|---|
| 400 | `invalid_request` | Битый JSON, нет файла, недопустимое значение параметра |
| 401 | `unauthorized` | Нет ключа или ключ неверный |
| 404 | `not_found` | Неизвестный путь |
| 413 | `payload_too_large` | Тело больше `PDF_INSPECTOR_MAX_UPLOAD_MB` |
| 422 | `not_a_pdf` | Тело не является PDF |
| 422 | `encrypted` | PDF зашифрован, пароль не подошёл |
| 422 | `parse_error` | PDF не разбирается |
| 422 | `invalid_structure` | Повреждена структура PDF |
| 500 | `internal_error` | Сбой ввода-вывода или падение рабочего потока |

Различие 4xx и 422 намеренное: 400 — проблема в самом запросе, 422 — запрос
корректен, но с документом работать нельзя.

---

## Настройки сервера

| Переменная | По умолчанию | Описание |
|---|---|---|
| `PDF_INSPECTOR_BIND` | `0.0.0.0:8080` | Адрес прослушивания |
| `PDF_INSPECTOR_PORT` / `PORT` | — | Переопределяет только порт |
| `PDF_INSPECTOR_API_KEY` | не задан | Включает проверку ключа на `/v1/*` |
| `PDF_INSPECTOR_MAX_UPLOAD_MB` | `64` | Предел размера запроса |
| `PDF_INSPECTOR_MAX_CONCURRENCY` | число ядер | Сколько PDF разбирается одновременно |
| `PDF_INSPECTOR_BCMAPS_DIR` | каталог крейта | Бинарные CMap'ы для CJK |
| `RUST_LOG` | `info` | Уровень логирования |

Те же значения задаются флагами: `--bind`, `--port`, `--max-upload-mb`,
`--concurrency` (флаги перекрывают переменные окружения).

Разбор PDF — нагрузка на процессор, поэтому каждый запрос выполняется в
отдельном блокирующем потоке, а их число ограничено семафором: всплеск
загрузок не породит неограниченное число потоков. Сервис не хранит состояние
и ничего не пишет на диск.

---

## Примеры клиентов

### Python

```python
import requests

BASE = "http://localhost:8080"
HEADERS = {"Authorization": "Bearer " + KEY}  # если включён ключ

with open("document.pdf", "rb") as fh:
    r = requests.post(
        f"{BASE}/v1/process",
        headers=HEADERS,
        files={"file": fh},
        data={"compact": "true", "pages": "1-5"},
        timeout=300,
    )
r.raise_for_status()
result = r.json()

if result["pages_needing_ocr"]:
    print("страницы под OCR:", result["pages_needing_ocr"])
print(result["markdown"])
```

### Node.js

```js
const form = new FormData();
form.set("file", new Blob([await readFile("document.pdf")]), "document.pdf");
form.set("options", JSON.stringify({ compact: true, pages: [1, 2] }));

const res = await fetch("http://localhost:8080/v1/process", {
  method: "POST",
  headers: { Authorization: `Bearer ${key}` },
  body: form,
});
if (!res.ok) throw new Error((await res.json()).error.message);
const { markdown, pdf_type } = await res.json();
```

### Маршрутизация «текст или OCR»

```bash
TYPE=$(curl -s -F file=@doc.pdf http://localhost:8080/v1/classify | jq -r .pdf_type)
case "$TYPE" in
  text_based) curl -s -F file=@doc.pdf 'http://localhost:8080/v1/process?format=markdown' ;;
  *)          echo "нужен OCR: $TYPE" ;;
esac
```
