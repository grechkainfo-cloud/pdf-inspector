# Развёртывание: Docker-образ и запуск как службы

Инструкция описывает полный цикл: сборка образа → выгрузка его в файл →
перенос на другой Linux-сервер → запуск как системной службы с проброшенным
HTTP-портом.

Описание самого API — в [`docs/http-api.md`](http-api.md).

---

## 0. Что входит в образ

| Компонент | Путь в контейнере |
|---|---|
| HTTP-сервер | `/usr/local/bin/pdf-api` |
| CLI-конвертер | `/usr/local/bin/pdf2md` |
| CLI-классификатор | `/usr/local/bin/detect-pdf` |
| Бинарные CMap'ы для CJK-шрифтов | `/opt/pdf-inspector/bcmaps` |

Образ двухстадийный: компиляция идёт в `rust:1-slim-bookworm`, а в финальный
`debian:bookworm-slim` попадают только собранные бинарники и CMap'ы. Ни
исходников, ни компилятора, ни доступа в сеть при работе не требуется.
Сервис не пишет на диск, работает от непривилегированного пользователя
(uid 10001) и слушает `0.0.0.0:8080` внутри контейнера.

Итоговый размер — примерно 110–130 МБ.

---

## 1. Сборка образа

На машине с Docker, из корня репозитория:

```bash
docker build -t pdf-inspector:0.1.7 -t pdf-inspector:latest .
```

Первая сборка занимает несколько минут (компиляция зависимостей Rust).
Повторные сборки после правок исходников — десятки секунд: слой с
зависимостями кешируется и переиспользуется, пока не изменится `Cargo.toml`.

Проверка сразу после сборки:

```bash
docker run --rm -d -p 8080:8080 --name pdf-check pdf-inspector:latest
sleep 2
curl -s http://localhost:8080/health          # {"status":"ok"}
curl -s -F file=@tests/fixtures/nexo-price-en.pdf http://localhost:8080/v1/detect
docker rm -f pdf-check
```

---

## 2. Выгрузка образа в файл для переноса

Способ подходит, когда на целевом сервере нет доступа к Docker-реестру
(закрытый контур, отсутствие интернета).

```bash
# Сохранить образ в архив. gzip уменьшает файл примерно вдвое.
docker save pdf-inspector:latest | gzip -9 > pdf-inspector-latest.tar.gz

# Контрольная сумма — чтобы убедиться, что файл доехал целым.
sha256sum pdf-inspector-latest.tar.gz > pdf-inspector-latest.tar.gz.sha256

ls -lh pdf-inspector-latest.tar.gz
```

> Если сохраняете несколько тегов одного образа, перечислите их в одной
> команде — слои не продублируются:
> `docker save pdf-inspector:0.1.7 pdf-inspector:latest | gzip -9 > ...`

### Windows / PowerShell

`docker save | gzip` в PowerShell портит бинарный поток, поэтому пишите в файл
напрямую, без конвейера:

```powershell
docker save -o pdf-inspector-latest.tar pdf-inspector:latest
# при необходимости сжать: 7z a pdf-inspector-latest.tar.gz pdf-inspector-latest.tar
```

---

## 3. Перенос на целевой сервер

```bash
scp pdf-inspector-latest.tar.gz \
    pdf-inspector-latest.tar.gz.sha256 \
    user@target-server:/tmp/
```

На целевом сервере:

```bash
cd /tmp
sha256sum -c pdf-inspector-latest.tar.gz.sha256   # должно быть: OK

# Загрузить образ в локальное хранилище Docker
gunzip -c pdf-inspector-latest.tar.gz | docker load

# Убедиться, что образ появился
docker images pdf-inspector
```

`docker load` выведет строку вида `Loaded image: pdf-inspector:latest` — это
имя и тег, которые дальше указываются в конфигурации службы.

### Альтернатива: через реестр

Если у серверов есть общий Docker-реестр (Docker Hub, GitLab Registry,
Harbor, ECR), перенос файлами не нужен:

```bash
# На сборочной машине
docker tag pdf-inspector:latest registry.example.com/tools/pdf-inspector:0.1.7
docker push registry.example.com/tools/pdf-inspector:0.1.7

# На целевом сервере
docker login registry.example.com
docker pull registry.example.com/tools/pdf-inspector:0.1.7
docker tag registry.example.com/tools/pdf-inspector:0.1.7 pdf-inspector:latest
```

---

## 4. Запуск как службы (systemd)

Вариант для «чистого» сервера с Docker, без docker compose. systemd владеет
жизненным циклом контейнера: перезапускает при падении, останавливает при
`systemctl stop`, пишет логи в журнал.

### 4.1. Установка

Файлы `deploy/pdf-inspector.service` и `deploy/pdf-inspector.env` лежат в
репозитории. Скопируйте их на сервер (например, тем же `scp`) и установите:

```bash
sudo cp pdf-inspector.service /etc/systemd/system/
sudo cp pdf-inspector.env     /etc/pdf-inspector.env
sudo chmod 600 /etc/pdf-inspector.env
sudo chown root:root /etc/pdf-inspector.env
```

### 4.2. Настройка

Отредактируйте `/etc/pdf-inspector.env`:

```ini
# Порт, который будет виден клиентам. 0.0.0.0 — на всех интерфейсах,
# 127.0.0.1 — только локально (когда впереди стоит nginx с TLS).
PDF_INSPECTOR_PUBLISH=0.0.0.0:8080

# Тег образа, загруженного на шаге 3.
PDF_INSPECTOR_IMAGE=pdf-inspector:latest

# Ключ доступа: без него /v1/* открыт всем, кто дотянется до порта.
PDF_INSPECTOR_API_KEY=<вывод команды openssl rand -hex 32>

PDF_INSPECTOR_MAX_UPLOAD_MB=64
RUST_LOG=info
```

Внутри контейнера сервис всегда слушает `0.0.0.0:8080`; наружу его выводит
проброс `--publish ${PDF_INSPECTOR_PUBLISH}:8080`. Менять
`PDF_INSPECTOR_BIND`/`PDF_INSPECTOR_PORT` в этом файле не нужно — это сломает
соответствие портов.

### 4.3. Запуск

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now pdf-inspector

sudo systemctl status pdf-inspector
sudo journalctl -u pdf-inspector -f      # логи запросов
```

Проверка снаружи:

```bash
curl -s http://<адрес-сервера>:8080/health
curl -s -H "Authorization: Bearer $KEY" \
     -F file=@document.pdf \
     http://<адрес-сервера>:8080/v1/process | jq .pdf_type
```

### 4.4. Управление

```bash
sudo systemctl restart pdf-inspector
sudo systemctl stop pdf-inspector
sudo systemctl disable --now pdf-inspector   # выключить автозапуск
```

### 4.5. Обновление версии

```bash
# 1. Перенести и загрузить новый образ (шаги 2–3), присвоив ему свой тег
gunzip -c pdf-inspector-0.1.8.tar.gz | docker load

# 2. Переключить службу на новый тег
sudo sed -i 's/^PDF_INSPECTOR_IMAGE=.*/PDF_INSPECTOR_IMAGE=pdf-inspector:0.1.8/' \
    /etc/pdf-inspector.env
sudo systemctl restart pdf-inspector

# 3. Откат — вернуть предыдущий тег и снова перезапустить
```

Старые образы удаляются командой `docker image prune -a` (осторожно: удалит
все образы, не связанные с запущенными контейнерами).

---

## 5. Запуск через docker compose

Альтернатива systemd-юниту, если на сервере уже используется compose. Файл
`docker-compose.yml` лежит в корне репозитория.

```bash
# С исходниками на сервере — собрать и запустить
docker compose up -d --build

# С заранее загруженным образом — убрать секцию build и указать image:
docker compose up -d

docker compose logs -f
docker compose down
```

Автозапуск обеспечивает `restart: unless-stopped` вместе с включённой службой
`docker.service`.

---

## 6. Порт, доступ и TLS

* **Порт.** Наружу публикуется только `8080/tcp`. Изменить внешний порт можно
  в `PDF_INSPECTOR_PUBLISH` (например, `0.0.0.0:9000` → API доступен на 9000).
* **Firewall.** Откройте порт явно:

  ```bash
  sudo ufw allow 8080/tcp                      # Ubuntu/Debian
  sudo firewall-cmd --add-port=8080/tcp --permanent && sudo firewall-cmd --reload
  ```

* **TLS.** Сервис говорит по HTTP. Для внешнего доступа поставьте перед ним
  обратный прокси, а публикацию сузьте до `127.0.0.1:8080`:

  ```nginx
  server {
      listen 443 ssl;
      server_name pdf.example.com;

      ssl_certificate     /etc/letsencrypt/live/pdf.example.com/fullchain.pem;
      ssl_certificate_key /etc/letsencrypt/live/pdf.example.com/privkey.pem;

      # Должно быть не меньше PDF_INSPECTOR_MAX_UPLOAD_MB.
      client_max_body_size 64m;
      # Разбор крупного PDF может занять десятки секунд.
      proxy_read_timeout 300s;

      location / {
          proxy_pass http://127.0.0.1:8080;
          proxy_set_header Host $host;
          proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
          proxy_set_header X-Forwarded-Proto $scheme;
      }
  }
  ```

* **Аутентификация.** `PDF_INSPECTOR_API_KEY` включает проверку ключа на всех
  `/v1/*`. `/health` остаётся открытым — иначе healthcheck и балансировщик не
  смогут опрашивать сервис.

---

## 7. Ресурсы и производительность

* Пиковая память ≈ размер самого крупного PDF × число одновременных разборов.
  `PDF_INSPECTOR_MAX_CONCURRENCY` ограничивает второй множитель; по умолчанию
  он равен числу ядер.
* Лимит памяти контейнера (`--memory 2g` в юните) не даёт одному запросу
  выбить сервер; при превышении процесс убивается OOM-killer'ом, а systemd
  поднимает контейнер заново.
* Сервис не хранит состояние: несколько экземпляров можно поставить за любым
  балансировщиком без общих ресурсов.

---

## 8. Диагностика

| Симптом | Причина и решение |
|---|---|
| `curl: connection refused` | Контейнер не запущен (`systemctl status pdf-inspector`) или порт не проброшен (проверьте `docker ps`). |
| `401 unauthorized` | Задан `PDF_INSPECTOR_API_KEY` — передайте `Authorization: Bearer <key>`. |
| `413 payload_too_large` | Файл больше `PDF_INSPECTOR_MAX_UPLOAD_MB`; поднимите лимит и, если стоит nginx, `client_max_body_size`. |
| `422 encrypted` | PDF зашифрован — передайте параметр `password`. |
| `422 not_a_pdf` | Тело запроса не PDF: при `-F` проверьте имя поля (`file`), при JSON — что `pdf_base64` содержит base64 файла. |
| Иероглифы в CJK-тексте | Потерян каталог CMap'ов. В образе он лежит в `/opt/pdf-inspector/bcmaps`, путь задаётся `PDF_INSPECTOR_BCMAPS_DIR` — не переопределяйте эту переменную. |
| Служба перезапускается по кругу | `journalctl -u pdf-inspector -n 100` покажет ошибку старта: чаще всего занят порт или отсутствует `/etc/pdf-inspector.env`. |

Подробные логи разбора включаются переменной `RUST_LOG`:

```bash
sudo sed -i 's/^RUST_LOG=.*/RUST_LOG=debug/' /etc/pdf-inspector.env
sudo systemctl restart pdf-inspector
```

---

## 9. Запуск без Docker

Если контейнеризация не нужна, бинарник ставится напрямую:

```bash
cargo build --release --features server --bin pdf-api
sudo install -m 755 target/release/pdf-api /usr/local/bin/pdf-api

# CMap'ы должны быть доступны по абсолютному пути
sudo mkdir -p /opt/pdf-inspector
sudo cp -r external/bcmaps /opt/pdf-inspector/
export PDF_INSPECTOR_BCMAPS_DIR=/opt/pdf-inspector/bcmaps

pdf-api --bind 0.0.0.0:8080
```

Для systemd в этом случае используется обычный `Type=simple`-юнит с
`ExecStart=/usr/local/bin/pdf-api` и `EnvironmentFile=/etc/pdf-inspector.env`.
