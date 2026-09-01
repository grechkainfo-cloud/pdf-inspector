# Развёртывание: Docker-образ и запуск как службы

Инструкция описывает полный цикл: перенос исходников → сборка образа →
выгрузка его в файл → перенос на другой Linux-сервер → запуск как системной
службы с проброшенным HTTP-портом.

Описание самого API — в [`docs/http-api.md`](http-api.md).

---

## 0. Что входит в образ

| Компонент | Путь в контейнере |
|---|---|
| HTTP-сервер | `/usr/local/bin/pdf-api` |
| CLI-конвертер | `/usr/local/bin/pdf2md` |
| CLI-классификатор | `/usr/local/bin/detect-pdf` |
| Бинарные CMap'ы для CJK-шрифтов | `/opt/pdf-inspector/bcmaps` |
| Тестовая консоль | встроена в бинарник, отдаётся на `/ui` |

Образ двухстадийный: компиляция идёт в `rust:1-slim-bookworm`, а в финальный
`debian:bookworm-slim` попадают только собранные бинарники и CMap'ы. Ни
исходников, ни компилятора, ни доступа в сеть при работе не требуется.
Сервис не пишет на диск, работает от непривилегированного пользователя
(uid 10001) и слушает `0.0.0.0:8080` внутри контейнера.

Итоговый размер — примерно 110–130 МБ.

---

## 1. Перенос исходников на сервер

Исходники на сервере нужны в трёх случаях: собрать образ прямо там, взять
файлы из `deploy/` для systemd-службы, запустить `deploy/cleanup.sh`. Если
образ уже собран на рабочей машине, полные исходники не нужны — хватит
раздела 1.5.

> **Сначала оцените свободное место.** Сборка образа на сервере требует
> 3–4 ГБ: слой `rust:1-slim-bookworm` весит около 1,5 ГБ, каталог `target/`
> внутри стадии сборки — ещё столько же. На забитом диске сборка упадёт на
> середине и оставит мусор в `/var/lib/docker`. Если на `/` меньше 5 ГБ
> свободного места — соберите образ на рабочей машине и перенесите готовым
> (разделы 3–4), а на сервер везите только файлы развёртывания.

### 1.1. Если сервер видит GitHub

```bash
git clone --depth 1 https://github.com/grechkainfo-cloud/pdf-inspector.git
cd pdf-inspector
```

`--depth 1` берёт один коммит вместо всей истории: 13 МБ каталога `.git`
на сервере не нужны. Нужна конкретная ветка — добавьте `-b <ветка>`.

Без git на сервере — тем же архивом прямо от GitHub:

```bash
curl -fsSL -o pdf-inspector.tar.gz \
    https://github.com/grechkainfo-cloud/pdf-inspector/archive/refs/heads/main.tar.gz
tar -xzf pdf-inspector.tar.gz    # распакуется в pdf-inspector-main/
```

### 1.2. Закрытый контур: архив с рабочей машины

Упаковывайте через `git archive`, а не архиватором по каталогу. Разница не
косметическая:

* архив собирается из коммита, а не из рабочей копии — внутрь не попадут
  `target/` (гигабайты), локальные правки и мусор редактора;
* переводы строк берутся из репозитория и остаются LF. Архиватор по
  рабочей копии на Windows везёт CRLF, а это `bad interpreter: ^M` у
  скриптов и висящий возврат каретки в каждом значении env-файла;
* бит `+x` на `deploy/cleanup.sh` сохраняется. Windows-раздел его вообще
  не хранит, поэтому `tar` по рабочей копии его теряет.

Работает одинаково в Git Bash, PowerShell и cmd:

```bash
# Полные исходники: 10 МБ, 331 файл
git archive --format=tar.gz --prefix=pdf-inspector/ -o pdf-inspector-src.tar.gz HEAD

# Контрольная сумма — проверить целостность после передачи
sha256sum pdf-inspector-src.tar.gz > pdf-inspector-src.tar.gz.sha256
```

`--prefix=` заставляет архив распаковаться в отдельный каталог, а не
рассыпаться 331 файлом по текущему.

Если на сервере нужно только собрать образ, берите минимум — Dockerfile
использует ровно эти пути, остальные 9 МБ приходятся на тестовые PDF:

```bash
# 1,4 МБ вместо 10
git archive --format=tar.gz --prefix=pdf-inspector/ \
    -o pdf-inspector-build.tar.gz HEAD Cargo.toml src external Dockerfile .dockerignore
```

**Чего не делать:** `Compress-Archive` и вообще zip. Формат не хранит
права Unix, поэтому `cleanup.sh` приедет без бита `+x`, а при упаковке
рабочей копии — ещё и с CRLF.

### 1.3. Передача

```bash
scp pdf-inspector-src.tar.gz pdf-inspector-src.tar.gz.sha256 user@target-server:/tmp/
```

Для повторных переносов удобнее `rsync` — он шлёт только изменившееся:

```bash
rsync -az --delete \
    --exclude '.git' --exclude 'target' --exclude 'tests/fixtures' \
    ./ user@target-server:/opt/pdf-inspector/
```

### 1.4. Распаковка и проверка

```bash
cd /tmp
sha256sum -c pdf-inspector-src.tar.gz.sha256   # должно быть: OK

sudo mkdir -p /opt && cd /opt
sudo tar -xzf /tmp/pdf-inspector-src.tar.gz
cd pdf-inspector
```

Две проверки, которые экономят час на отладке непонятных сбоев:

```bash
# 1. Права: у скрипта должен быть бит +x
ls -l deploy/cleanup.sh          # -rwxr-xr-x ... deploy/cleanup.sh

# 2. Переводы строк: в выводе не должно быть "CRLF line terminators"
file deploy/cleanup.sh deploy/pdf-inspector.env Dockerfile
```

Если что-то приехало не так (паковали zip'ом или правили в
Windows-редакторе):

```bash
# CRLF → LF
sudo dnf install -y dos2unix        # или: sudo apt-get install -y dos2unix
dos2unix deploy/*.sh deploy/*.env deploy/*.service Dockerfile
# без dos2unix то же самое:
sed -i 's/\r$//' deploy/cleanup.sh deploy/pdf-inspector.env

# потерянный бит выполнения
chmod +x deploy/cleanup.sh
```

Быстрая проверка, что исходники приехали целыми:

```bash
ls Cargo.toml src/lib.rs external/bcmaps/ | head
```

Каталог `external/bcmaps` обязателен: без него сборка пройдёт, но
контейнер потеряет Unicode-отображение CJK-шрифтов с CID-кодировкой.

### 1.5. Только файлы развёртывания

Когда образ собран заранее и переносится готовым, исходники не нужны —
на сервер едут 12 КБ:

```bash
git archive --format=tar.gz --prefix=pdf-inspector/ \
    -o pdf-deploy.tar.gz HEAD deploy docker-compose.yml
```

Внутри — `pdf-inspector.service`, `pdf-inspector.env`, `cleanup.sh` и
`docker-compose.yml`, то есть всё, что нужно разделам 5 и 6.

### 1.6. Сколько это занимает

| Что | Размер |
|---|---|
| Архив исходников, полный | 10 МБ |
| Он же распакованный | 15 МБ |
| Архив только для сборки образа | 1,4 МБ |
| Архив только файлов развёртывания | 12 КБ |
| Свободное место под сборку образа | 3–4 ГБ |
| Готовый образ | 110–130 МБ |

---

## 2. Сборка образа

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

Проще всего проверить работу глазами — открыть тестовую консоль
<http://localhost:8080/ui>, перетащить туда PDF и нажать «Выполнить».

---

## 3. Выгрузка образа в файл для переноса

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

## 4. Перенос на целевой сервер

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

## 5. Запуск как службы (systemd)

Вариант для «чистого» сервера с Docker, без docker compose. systemd владеет
жизненным циклом контейнера: перезапускает при падении, останавливает при
`systemctl stop`, пишет логи в журнал.

### 5.1. Установка

Файлы `deploy/pdf-inspector.service` и `deploy/pdf-inspector.env` приезжают
вместе с исходниками (раздел 1). Из корня распакованного каталога:

```bash
sudo cp deploy/pdf-inspector.service /etc/systemd/system/
sudo cp deploy/pdf-inspector.env     /etc/pdf-inspector.env
sudo chmod 600 /etc/pdf-inspector.env
sudo chown root:root /etc/pdf-inspector.env
```

### 5.2. Настройка

Отредактируйте `/etc/pdf-inspector.env`:

```ini
# Порт, который будет виден клиентам. 0.0.0.0 — на всех интерфейсах,
# 127.0.0.1 — только локально (когда впереди стоит nginx с TLS).
PDF_INSPECTOR_PUBLISH=0.0.0.0:8080

# Тег образа, загруженного на шаге 4.
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

### 5.3. Запуск

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

### 5.4. Управление

```bash
sudo systemctl restart pdf-inspector
sudo systemctl stop pdf-inspector
sudo systemctl disable --now pdf-inspector   # выключить автозапуск
```

### 5.5. Обновление версии

```bash
# 1. Перенести и загрузить новый образ (шаги 3–4), присвоив ему свой тег
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

## 6. Запуск через docker compose

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

## 7. Порт, доступ и TLS

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

## 8. Ресурсы и производительность

* Пиковая память ≈ размер самого крупного PDF × число одновременных разборов.
  `PDF_INSPECTOR_MAX_CONCURRENCY` ограничивает второй множитель; по умолчанию
  он равен числу ядер.
* Лимит памяти контейнера (`--memory 2g` в юните) не даёт одному запросу
  выбить сервер; при превышении процесс убивается OOM-killer'ом, а systemd
  поднимает контейнер заново.
* Сервис не хранит состояние: несколько экземпляров можно поставить за любым
  балансировщиком без общих ресурсов.

---

## 9. Диагностика

| Симптом | Причина и решение |
|---|---|
| `No such container: pdf-inspector` в журнале | Сообщение `ExecStartPre`/`ExecStop`: юнит на всякий случай удаляет контейнер, оставшийся от прошлого запуска, а удалять нечего. На работу не влияет — важно лишь состояние службы (`systemctl is-active pdf-inspector`, `docker ps`). В свежей версии `deploy/pdf-inspector.service` это сообщение погашено. |
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

## 10. Запуск без Docker

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
