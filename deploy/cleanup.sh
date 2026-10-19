#!/usr/bin/env bash
#
# cleanup.sh — анализ и очистка дискового пространства на Linux-сервере.
#
# По умолчанию скрипт НИЧЕГО не удаляет: он только собирает отчёт и говорит,
# сколько и где можно освободить. Удаление включается флагом --clean, а всё
# по-настоящему опасное — отдельными флагами поверх него.
#
#   ./cleanup.sh                      отчёт (только чтение, безопасно везде)
#   ./cleanup.sh --clean --dry-run    показать, что удалил бы, ничего не трогая
#   ./cleanup.sh --clean              безопасная очистка
#   ./cleanup.sh --clean --docker-images --docker-logs
#   ./cleanup.sh --clean --all --yes  всё, кроме docker-томов, без вопросов
#
# Поддерживает dnf/yum/apt/zypper/apk и systemd-journald.
# Зависимости: bash 4+, coreutils, findutils. lsof и docker — опционально.

set -uo pipefail

VERSION="1.0.0"

# --- Настройки по умолчанию (переопределяются флагами) ----------------------

TARGET="/"              # какую файловую систему разбираем
TOP=15                  # длина топ-списков
MIN_FILE_MB=100         # порог «крупного файла»
JOURNAL_KEEP="200M"     # сколько журнала systemd оставить
LOG_KEEP_DAYS=7         # возраст ротированных логов под удаление
TMP_KEEP_DAYS=7         # возраст файлов в /tmp и /var/tmp под удаление
DOCKER_LOG_KEEP="10M"   # до какого размера обрезать логи контейнеров
KERNELS_KEEP=2          # сколько версий ядра оставить
HELD_MIN_MB=10          # порог для «удалённых, но открытых» файлов

DRY_RUN=0
DO_CLEAN=0
ASSUME_YES=0
CLEAN_DOCKER_IMAGES=0
CLEAN_DOCKER_LOGS=0
CLEAN_DOCKER_CONTAINERS=0
CLEAN_KERNELS=0
TRUNCATE_LOGS=0

RECLAIM_BYTES=0         # накопленная оценка «сколько можно освободить»
declare -a HINTS=()     # рекомендации для итогового блока

# --- Оформление -------------------------------------------------------------

if [[ -t 1 ]] && command -v tput >/dev/null 2>&1 && [[ $(tput colors 2>/dev/null || echo 0) -ge 8 ]]; then
    C_RESET=$(tput sgr0); C_BOLD=$(tput bold)
    C_RED=$(tput setaf 1); C_GREEN=$(tput setaf 2)
    C_YELLOW=$(tput setaf 3); C_BLUE=$(tput setaf 4)
else
    C_RESET=""; C_BOLD=""; C_RED=""; C_GREEN=""; C_YELLOW=""; C_BLUE=""
fi

section() { printf '\n%s%s== %s%s\n' "$C_BOLD" "$C_BLUE" "$1" "$C_RESET"; }
info()    { printf '   %s\n' "$*"; }
ok()      { printf '   %s✓%s %s\n' "$C_GREEN" "$C_RESET" "$*"; }
warn()    { printf '   %s!%s %s\n' "$C_YELLOW" "$C_RESET" "$*"; }
err()     { printf '   %sx%s %s\n' "$C_RED" "$C_RESET" "$*" >&2; }

have() { command -v "$1" >/dev/null 2>&1; }

usage() {
    cat <<'EOF'
cleanup.sh — анализ и очистка дискового пространства

ИСПОЛЬЗОВАНИЕ
    cleanup.sh [ОПЦИИ]

РЕЖИМ
    (без флагов)          только отчёт, ничего не удаляется
    --clean               выполнить безопасную очистку
    --dry-run             с --clean: показать команды, не выполняя их
    -y, --yes             не спрашивать подтверждение

ДОПОЛНИТЕЛЬНАЯ ОЧИСТКА (только вместе с --clean)
    --docker-images       удалить неиспользуемые образы Docker (не только висячие)
    --docker-containers   удалить остановленные контейнеры
    --docker-logs         обрезать логи контейнеров до --docker-log-keep
    --truncate-logs       обнулить активные логи крупнее 100MB в /var/log
    --kernels             удалить старые ядра, оставив --kernels-keep штук
    --all                 всё перечисленное выше (тома Docker НЕ трогаются)

ПАРАМЕТРЫ
    --target ПУТЬ         анализируемая ФС (по умолчанию /)
    --top N               длина топ-списков (по умолчанию 15)
    --min-size МБ         порог крупного файла (по умолчанию 100)
    --journal-keep РАЗМЕР сколько журнала оставить (по умолчанию 200M)
    --log-days N          возраст ротированных логов под удаление (7)
    --tmp-days N          возраст файлов во временных каталогах (7)
    --docker-log-keep Р   до какого размера обрезать логи контейнеров (10M)
    --kernels-keep N      сколько ядер оставить (2)

    -h, --help            эта справка
    -V, --version         версия

ЧТО СЧИТАЕТСЯ БЕЗОПАСНЫМ (входит в --clean)
    Кэши пакетного менеджера, журнал systemd сверх лимита, висячие образы и
    кэш сборки Docker, ротированные логи старше N дней, старые файлы во
    временных каталогах, дампы памяти.

ЧТО НИКОГДА НЕ УДАЛЯЕТСЯ
    Тома Docker (docker volume prune) — там живут данные приложений.
    Работающие контейнеры и используемые ими образы.
    Файлы в домашних каталогах пользователей.
EOF
}

# --- Утилиты ----------------------------------------------------------------

# Байты → человекочитаемый вид.
human() {
    local bytes=${1:-0}
    [[ $bytes =~ ^-?[0-9]+$ ]] || bytes=0
    awk -v b="$bytes" 'BEGIN {
        sign = ""; if (b < 0) { sign = "-"; b = -b }
        split("B KiB MiB GiB TiB PiB", u, " "); i = 1
        while (b >= 1024 && i < 6) { b /= 1024; i++ }
        printf "%s%.1f%s", sign, b, u[i]
    }'
}

# «200M», «1.5G», «1024» → байты.
to_bytes() {
    local value=${1:-0} num unit mult=1
    [[ $value =~ ^([0-9]+([.][0-9]+)?)[[:space:]]*([A-Za-z]*)$ ]] || { echo 0; return; }
    num=${BASH_REMATCH[1]}
    unit=${BASH_REMATCH[3]}
    case ${unit^^} in
        ""|B)     mult=1 ;;
        K|KB|KIB) mult=1024 ;;
        M|MB|MIB) mult=$(( 1024 ** 2 )) ;;
        G|GB|GIB) mult=$(( 1024 ** 3 )) ;;
        T|TB|TIB) mult=$(( 1024 ** 4 )) ;;
        *)        mult=1 ;;
    esac
    awk -v n="$num" -v m="$mult" 'BEGIN { printf "%d", n * m }'
}

# Размер каталога в байтах; 0, если каталога нет или он недоступен.
dir_bytes() {
    local path=$1 out
    [[ -e $path ]] || { echo 0; return; }
    out=$(du -sx --block-size=1 "$path" 2>/dev/null | awk 'NR==1 {print $1}')
    [[ $out =~ ^[0-9]+$ ]] && echo "$out" || echo 0
}

add_reclaim() {
    local bytes=${1:-0}
    [[ $bytes =~ ^[0-9]+$ ]] || return 0
    RECLAIM_BYTES=$(( RECLAIM_BYTES + bytes ))
}

hint() { HINTS+=("$1"); }

# Выполнить команду с учётом --dry-run.
run() {
    if (( DRY_RUN )); then
        printf '   %s[dry-run]%s %s\n' "$C_YELLOW" "$C_RESET" "$*"
        return 0
    fi
    printf '   → %s\n' "$*"
    "$@"
}

# То же для конструкций с конвейерами и подстановками.
run_sh() {
    if (( DRY_RUN )); then
        printf '   %s[dry-run]%s %s\n' "$C_YELLOW" "$C_RESET" "$1"
        return 0
    fi
    printf '   → %s\n' "$1"
    bash -c "$1"
}

is_root() { [[ ${EUID:-$(id -u)} -eq 0 ]]; }

confirm() {
    (( ASSUME_YES )) && return 0
    (( DRY_RUN )) && return 0
    local reply
    printf '\n%s%s%s [y/N] ' "$C_BOLD" "$1" "$C_RESET"
    read -r reply </dev/tty 2>/dev/null || {
        err "нет терминала для подтверждения — добавьте --yes"
        return 1
    }
    [[ $reply =~ ^[YyДд]$ ]]
}

avail_bytes() {
    df -P --block-size=1 "$TARGET" 2>/dev/null | awk 'NR==2 {print $4}'
}

# --- Определение пакетного менеджера ---------------------------------------

PKG_MGR=""
detect_pkg_mgr() {
    if   have dnf;     then PKG_MGR=dnf
    elif have yum;     then PKG_MGR=yum
    elif have apt-get; then PKG_MGR=apt
    elif have zypper;  then PKG_MGR=zypper
    elif have apk;     then PKG_MGR=apk
    fi
}

# --- Разбор аргументов ------------------------------------------------------

parse_args() {
    while (( $# )); do
        case $1 in
            --clean)             DO_CLEAN=1 ;;
            --dry-run)           DRY_RUN=1 ;;
            -y|--yes)            ASSUME_YES=1 ;;
            --docker-images)     CLEAN_DOCKER_IMAGES=1 ;;
            --docker-containers) CLEAN_DOCKER_CONTAINERS=1 ;;
            --docker-logs)       CLEAN_DOCKER_LOGS=1 ;;
            --truncate-logs)     TRUNCATE_LOGS=1 ;;
            --kernels)           CLEAN_KERNELS=1 ;;
            --all)
                CLEAN_DOCKER_IMAGES=1
                CLEAN_DOCKER_CONTAINERS=1
                CLEAN_DOCKER_LOGS=1
                CLEAN_KERNELS=1
                TRUNCATE_LOGS=1
                ;;
            --target)          TARGET=${2:?--target требует значение}; shift ;;
            --top)             TOP=${2:?--top требует значение}; shift ;;
            --min-size)        MIN_FILE_MB=${2:?--min-size требует значение}; shift ;;
            --journal-keep)    JOURNAL_KEEP=${2:?--journal-keep требует значение}; shift ;;
            --log-days)        LOG_KEEP_DAYS=${2:?--log-days требует значение}; shift ;;
            --tmp-days)        TMP_KEEP_DAYS=${2:?--tmp-days требует значение}; shift ;;
            --docker-log-keep) DOCKER_LOG_KEEP=${2:?--docker-log-keep требует значение}; shift ;;
            --kernels-keep)    KERNELS_KEEP=${2:?--kernels-keep требует значение}; shift ;;
            -h|--help)         usage; exit 0 ;;
            -V|--version)      echo "cleanup.sh $VERSION"; exit 0 ;;
            *) err "неизвестный аргумент: $1"; echo; usage; exit 2 ;;
        esac
        shift
    done

    if (( DRY_RUN && ! DO_CLEAN )); then
        warn "--dry-run имеет смысл только с --clean; продолжаю в режиме отчёта"
    fi
}

# ===========================================================================
# АНАЛИЗ
# ===========================================================================

analyze_filesystems() {
    section "Файловые системы"
    df -hT -x tmpfs -x devtmpfs -x squashfs -x overlay 2>/dev/null | sed 's/^/   /'

    # Забитые inode выглядят как «нет места», хотя df -h показывает свободное.
    section "Inode"
    local inode_out
    inode_out=$(df -i -x tmpfs -x devtmpfs -x squashfs -x overlay 2>/dev/null)
    echo "$inode_out" | sed 's/^/   /'
    if echo "$inode_out" | awk 'NR>1 { gsub(/%/,"",$5); if ($5+0 >= 90) found=1 } END { exit !found }'; then
        warn "есть ФС с использованием inode ≥90% — место не вернётся удалением крупных файлов"
        hint "Ищите каталоги с сотнями тысяч мелких файлов: find / -xdev -type d -size +1M"
    fi

    local avail
    avail=$(avail_bytes)
    if [[ -n ${avail:-} ]] && (( avail < 512 * 1024 * 1024 )); then
        warn "на $TARGET осталось $(human "$avail") — dnf, docker build и запись логов уже могут падать"
        hint "Быстрые победы при нуле свободного места: journalctl --vacuum-size=100M, затем docker builder prune -f"
    fi
}

analyze_top_dirs() {
    section "Крупнейшие каталоги на $TARGET (первый уровень)"
    du -xh --max-depth=1 "$TARGET" 2>/dev/null | sort -rh | head -n "$(( TOP + 1 ))" | sed 's/^/   /'

    section "Крупнейшие каталоги (второй уровень)"
    du -xh --max-depth=2 "$TARGET" 2>/dev/null | sort -rh | sed -n "2,$(( TOP + 1 ))p" | sed 's/^/   /'
}

analyze_big_files() {
    section "Файлы крупнее ${MIN_FILE_MB}MB"
    local found=0 size path
    while IFS=$'\t' read -r size path; do
        [[ -n ${size:-} ]] || continue
        printf '   %10s  %s\n' "$(human "$size")" "$path"
        found=1
    done < <(find "$TARGET" -xdev -type f -size +"${MIN_FILE_MB}"M -printf '%s\t%p\n' 2>/dev/null \
             | sort -rn | head -n "$TOP")
    (( found )) || info "не найдено"
}

# Классическая причина «df показывает 100%, а du — заметно меньше»: файл
# удалён, но процесс держит открытый дескриптор, и место вернётся только
# после перезапуска этого процесса. Читаем /proc напрямую: точнее lsof и не
# зависит от его версии и раскладки колонок.
analyze_held_files() {
    section "Удалённые файлы, удерживаемые процессами"

    if ! is_root; then
        info "нужен root, чтобы увидеть дескрипторы чужих процессов — проверка неполная"
    fi

    local list total=0 count=0 min_bytes=$(( HELD_MIN_MB * 1024 * 1024 ))
    list=$(mktemp "${TMPDIR:-/tmp}/cleanup.XXXXXX") || { warn "не удалось создать временный файл"; return 0; }

    local fd target size inode pid comm
    for fd in /proc/[0-9]*/fd/*; do
        target=$(readlink "$fd" 2>/dev/null) || continue
        [[ $target == *"(deleted)" ]] || continue
        read -r size inode < <(stat -L -c '%s %i' "$fd" 2>/dev/null) || continue
        [[ $size =~ ^[0-9]+$ ]] || continue
        (( size >= min_bytes )) || continue
        pid=${fd#/proc/}; pid=${pid%%/*}
        comm=$(tr -d '\n' < "/proc/$pid/comm" 2>/dev/null) || comm="?"
        # Один и тот же inode виден через несколько дескрипторов — место
        # занято один раз, поэтому дедуплицируем по нему.
        printf '%s\t%s\t%s\t%s\t%s\n' "$inode" "$size" "$pid" "${comm:-?}" "${target% (deleted)}"
    done | sort -u -k1,1 > "$list"

    while IFS=$'\t' read -r inode size pid comm path; do
        [[ -n ${size:-} ]] || continue
        total=$(( total + size ))
        count=$(( count + 1 ))
        (( count <= TOP )) && printf '   %10s  pid %-7s %-16s %s\n' "$(human "$size")" "$pid" "$comm" "$path"
    done < <(sort -t$'\t' -k2,2 -rn "$list")

    rm -f "$list"

    if (( count )); then
        (( count > TOP )) && info "... и ещё $(( count - TOP ))"
        warn "процессы удерживают примерно $(human "$total") — место вернётся после их перезапуска"
        hint "Перезапустите перечисленные службы: systemctl restart <служба>. Файлы удалять не нужно"
        add_reclaim "$total"
    else
        ok "не обнаружено"
    fi
}

analyze_journal() {
    have journalctl || return 0
    section "Журнал systemd"
    info "$(journalctl --disk-usage 2>/dev/null || echo 'размер недоступен')"

    local bytes keep
    bytes=$(dir_bytes /var/log/journal)
    keep=$(to_bytes "$JOURNAL_KEEP")

    if (( bytes > keep )); then
        local excess=$(( bytes - keep ))
        warn "журнал занимает $(human "$bytes"), сверх лимита $JOURNAL_KEEP — $(human "$excess")"
        add_reclaim "$excess"
        hint "journalctl --vacuum-size=$JOURNAL_KEEP освободит около $(human "$excess")"
    else
        ok "журнал занимает $(human "$bytes") — в пределах лимита"
    fi
}

analyze_pkg_cache() {
    section "Кэш пакетного менеджера (${PKG_MGR:-не определён})"
    local bytes=0
    case $PKG_MGR in
        dnf|yum) bytes=$(( $(dir_bytes /var/cache/dnf) + $(dir_bytes /var/cache/yum) )) ;;
        apt)     bytes=$(( $(dir_bytes /var/cache/apt) + $(dir_bytes /var/lib/apt/lists) )) ;;
        zypper)  bytes=$(dir_bytes /var/cache/zypp) ;;
        apk)     bytes=$(dir_bytes /var/cache/apk) ;;
        *)       info "пропускаю"; return 0 ;;
    esac

    if (( bytes > 50 * 1024 * 1024 )); then
        warn "кэш занимает $(human "$bytes")"
        add_reclaim "$bytes"
    else
        ok "кэш занимает $(human "$bytes")"
    fi

    if have snap; then
        local snap_bytes
        snap_bytes=$(dir_bytes /var/lib/snapd/snaps)
        if (( snap_bytes > 100 * 1024 * 1024 )); then
            warn "snap занимает $(human "$snap_bytes"), включая отключённые ревизии"
            hint "Удалите старые ревизии snap: snap list --all | awk '/disabled/{print \$1, \$3}'"
        fi
    fi
}

analyze_kernels() {
    section "Установленные ядра"
    info "текущее: $(uname -r)"

    local installed="" count
    case $PKG_MGR in
        dnf|yum) installed=$(rpm -qa 2>/dev/null | grep -E '^kernel(-core)?-[0-9]' | sort -V) ;;
        apt)     installed=$(dpkg-query -W -f='${Package}\n' 'linux-image-[0-9]*' 2>/dev/null | sort -V) ;;
        *)       info "проверка ядер для ${PKG_MGR:-этой системы} не реализована"; return 0 ;;
    esac

    if [[ -z $installed ]]; then
        info "список ядер получить не удалось"
        return 0
    fi
    echo "$installed" | sed 's/^/   /'
    count=$(echo "$installed" | grep -c .)

    if (( count > KERNELS_KEEP )); then
        # Ядро с модулями — порядка 300MB в /usr/lib/modules плюс initramfs.
        local excess=$(( (count - KERNELS_KEEP) * 300 * 1024 * 1024 ))
        warn "установлено ядер: $count, оставить нужно $KERNELS_KEEP — примерно $(human "$excess") лишних"
        add_reclaim "$excess"
        hint "Удалить старые ядра: sudo $0 --clean --kernels"
    else
        ok "лишних ядер нет"
    fi

    local boot_use
    boot_use=$(df -P /boot 2>/dev/null | awk 'NR==2 { gsub(/%/,"",$5); print $5 }')
    if [[ ${boot_use:-0} =~ ^[0-9]+$ ]] && (( boot_use >= 80 )); then
        warn "/boot заполнен на ${boot_use}% — следующее обновление ядра может не пройти"
    fi
}

analyze_docker() {
    section "Docker"
    if ! have docker; then
        info "docker не установлен"
        return 0
    fi
    if ! docker info >/dev/null 2>&1; then
        warn "демон Docker недоступен (не запущен или нет прав) — раздел пропущен"
        return 0
    fi

    docker system df 2>/dev/null | sed 's/^/   /'

    # RECLAIMABLE из docker system df — самая точная оценка. Берём её через
    # --format: колонки в табличном выводе разъезжаются («Local Volumes»,
    # «Build Cache» содержат пробел, а процент есть не в каждой строке).
    local reclaimable=0 value
    while read -r value; do
        [[ -n ${value:-} ]] || continue
        # «800MB (66%)» → «800MB»
        value=${value%% *}
        reclaimable=$(( reclaimable + $(to_bytes "$value") ))
    done < <(docker system df --format '{{.Reclaimable}}' 2>/dev/null)

    if (( reclaimable > 0 )); then
        warn "Docker сообщает о $(human "$reclaimable") освобождаемого места"
        add_reclaim "$reclaimable"
    fi

    info "весь /var/lib/docker: $(human "$(dir_bytes /var/lib/docker)")"

    local dangling stopped volumes
    dangling=$(docker images -f dangling=true -q 2>/dev/null | grep -c .)
    stopped=$(docker ps -aq -f status=exited -f status=created 2>/dev/null | grep -c .)
    volumes=$(docker volume ls -qf dangling=true 2>/dev/null | grep -c .)

    (( dangling > 0 )) && warn "висячих образов: $dangling"
    (( stopped > 0 ))  && warn "остановленных контейнеров: $stopped"
    if (( volumes > 0 )); then
        warn "неиспользуемых томов: $volumes — скрипт их не трогает"
        hint "Тома проверьте вручную (docker volume ls -f dangling=true): в них могут быть данные приложений"
    fi

    section "Docker: крупнейшие образы"
    docker images --format '{{.Size}}\t{{.Repository}}:{{.Tag}}' 2>/dev/null \
        | sort -rh | head -n "$TOP" | sed 's/^/   /'

    section "Docker: логи контейнеров"
    local log_total=0 big=0 size path cid name
    while IFS=$'\t' read -r size path; do
        [[ $size =~ ^[0-9]+$ ]] || continue
        log_total=$(( log_total + size ))
        if (( size > 10 * 1024 * 1024 )); then
            cid=$(basename "$(dirname "$path")")
            name=$(docker inspect --format '{{.Name}}' "$cid" 2>/dev/null | sed 's|^/||')
            printf '   %10s  %s\n' "$(human "$size")" "${name:-${cid:0:12}}"
            big=1
        fi
    done < <(find /var/lib/docker/containers -name '*-json.log' -printf '%s\t%p\n' 2>/dev/null | sort -rn)

    if (( big )); then
        warn "логи контейнеров занимают $(human "$log_total")"
        add_reclaim "$log_total"
        hint "Обрежьте логи (sudo $0 --clean --docker-logs), а чтобы не росли снова — задайте max-size и max-file в настройках логирования"
    else
        ok "логи контейнеров занимают $(human "$log_total")"
    fi
}

analyze_logs() {
    section "Логи (/var/log)"
    info "всего: $(human "$(dir_bytes /var/log)")"
    du -xh --max-depth=1 /var/log 2>/dev/null | sort -rh | head -n "$TOP" | sed 's/^/   /'

    local rotated=0 size
    while read -r size; do
        [[ $size =~ ^[0-9]+$ ]] && rotated=$(( rotated + size ))
    done < <(find /var/log -type f \( -name '*.gz' -o -name '*.xz' -o -name '*.[0-9]' -o -name '*.old' \) \
             -printf '%s\n' 2>/dev/null)

    if (( rotated > 20 * 1024 * 1024 )); then
        warn "ротированные логи занимают $(human "$rotated")"
        add_reclaim "$rotated"
    else
        ok "ротированные логи занимают $(human "$rotated")"
    fi
}

analyze_tmp() {
    section "Временные каталоги и дампы"
    local path bytes fstype
    for path in /tmp /var/tmp /var/crash /var/lib/systemd/coredump; do
        [[ -d $path ]] || continue
        bytes=$(dir_bytes "$path")
        fstype=$(df -PT "$path" 2>/dev/null | awk 'NR==2 {print $2}')
        if [[ $fstype == tmpfs ]]; then
            printf '   %10s  %s (tmpfs — в ОЗУ, на диск не влияет)\n' "$(human "$bytes")" "$path"
        else
            printf '   %10s  %s\n' "$(human "$bytes")" "$path"
            (( bytes > 50 * 1024 * 1024 )) && add_reclaim "$bytes"
        fi
    done

    if have coredumpctl; then
        local dumps
        dumps=$(coredumpctl list --no-pager 2>/dev/null | grep -cE '^[0-9]{4}-')
        (( dumps > 0 )) && warn "дампов памяти: $dumps"
    fi
}

analyze_homes() {
    section "Домашние каталоги"
    du -xh --max-depth=1 /home /root 2>/dev/null | sort -rh | head -n "$TOP" | sed 's/^/   /'
    info "скрипт их не очищает — только показывает"
}

report_summary() {
    section "Итог"
    info "свободно сейчас: $(human "$(avail_bytes)")"
    info "оценка освобождаемого: примерно $(human "$RECLAIM_BYTES")"

    if (( ${#HINTS[@]} )); then
        printf '\n   %sЧто сделать:%s\n' "$C_BOLD" "$C_RESET"
        local i=1 h
        for h in "${HINTS[@]}"; do
            printf '   %d. %s\n' "$i" "$h"
            i=$(( i + 1 ))
        done
    fi

    if (( ! DO_CLEAN )); then
        printf '\n   Посмотреть план очистки:  %ssudo %s --clean --dry-run%s\n' "$C_BOLD" "$0" "$C_RESET"
        printf '   Выполнить очистку:        %ssudo %s --clean%s\n' "$C_BOLD" "$0" "$C_RESET"
    fi
}

# ===========================================================================
# ОЧИСТКА
# ===========================================================================

clean_journal() {
    have journalctl || return 0
    section "Очистка: журнал systemd"
    run journalctl --rotate
    run journalctl --vacuum-size="$JOURNAL_KEEP"
}

clean_pkg_cache() {
    section "Очистка: кэш пакетов"
    case $PKG_MGR in
        dnf)    run dnf clean all ;;
        yum)    run yum clean all ;;
        apt)    run apt-get clean
                run apt-get autoclean -y ;;
        zypper) run zypper clean --all ;;
        apk)    run_sh 'rm -rf /var/cache/apk/*' ;;
        *)      info "пакетный менеджер не определён — пропускаю" ;;
    esac
}

clean_logs() {
    section "Очистка: ротированные логи старше ${LOG_KEEP_DAYS} дн."
    run_sh "find /var/log -type f \\( -name '*.gz' -o -name '*.xz' -o -name '*.[0-9]' -o -name '*.old' \\) -mtime +${LOG_KEEP_DAYS} -print -delete"

    if (( TRUNCATE_LOGS )); then
        # Обнуляем, а не удаляем: удаление активного лога не вернёт место,
        # пока писавшая в него служба не перезапустится.
        section "Очистка: обнуление активных логов крупнее 100MB"
        run_sh "find /var/log -maxdepth 2 -type f -name '*.log' -size +100M -print -exec truncate -s 0 {} \\;"
    else
        info "активные логи не трогаю (включить: --truncate-logs)"
    fi
}

clean_tmp() {
    section "Очистка: временные файлы старше ${TMP_KEEP_DAYS} дн."
    local path
    for path in /tmp /var/tmp; do
        [[ -d $path ]] || continue
        # Только обычные файлы: сокеты и каталоги служб (systemd-private-*)
        # трогать нельзя — это сломает работающие сервисы.
        run_sh "find $path -xdev -mindepth 1 -type f -atime +${TMP_KEEP_DAYS} -mtime +${TMP_KEEP_DAYS} ! -path '*/systemd-private-*' -print -delete 2>/dev/null || true"
    done
}

clean_coredumps() {
    section "Очистка: дампы памяти"
    [[ -d /var/lib/systemd/coredump ]] && run_sh "find /var/lib/systemd/coredump -type f -print -delete"
    [[ -d /var/crash ]] && run_sh "find /var/crash -type f -mtime +1 -print -delete"
    return 0
}

clean_docker() {
    have docker || return 0
    docker info >/dev/null 2>&1 || { warn "демон Docker недоступен — пропускаю"; return 0; }

    section "Очистка: Docker"
    # Кэш сборки обычно самый крупный и всегда воссоздаётся — забираем всегда.
    run docker builder prune -f
    # Только висячие образы: используемые остаются на месте.
    run docker image prune -f
    run docker network prune -f

    if (( CLEAN_DOCKER_CONTAINERS )); then
        run docker container prune -f
    else
        info "остановленные контейнеры оставлены (включить: --docker-containers)"
    fi

    if (( CLEAN_DOCKER_IMAGES )); then
        warn "удаляю все образы, не привязанные к контейнерам"
        run docker image prune -a -f
    else
        info "неиспользуемые образы оставлены (включить: --docker-images)"
    fi

    info "тома Docker не трогаю — проверьте их вручную"
}

clean_docker_logs() {
    (( CLEAN_DOCKER_LOGS )) || return 0
    have docker || return 0

    section "Очистка: логи контейнеров (обрезаю до $DOCKER_LOG_KEEP)"
    local count=0 size path
    while IFS=$'\t' read -r size path; do
        [[ $size =~ ^[0-9]+$ ]] || continue
        (( size > $(to_bytes "$DOCKER_LOG_KEEP") )) || continue
        run truncate -s "$DOCKER_LOG_KEEP" "$path"
        count=$(( count + 1 ))
    done < <(find /var/lib/docker/containers -name '*-json.log' -printf '%s\t%p\n' 2>/dev/null)

    if (( count )); then
        ok "обрезано файлов: $count"
        info "чтобы логи не росли снова: --log-opt max-size=10m --log-opt max-file=3"
    else
        info "нечего обрезать"
    fi
}

clean_kernels() {
    (( CLEAN_KERNELS )) || return 0
    section "Очистка: старые ядра (оставляю $KERNELS_KEEP)"
    case $PKG_MGR in
        dnf) run dnf remove --oldinstallonly --setopt installonly_limit="$KERNELS_KEEP" -y ;;
        yum)
            if have package-cleanup; then
                run package-cleanup --oldkernels --count="$KERNELS_KEEP" -y
            else
                warn "package-cleanup недоступен (dnf install yum-utils) — пропускаю"
            fi ;;
        apt) run apt-get autoremove --purge -y ;;
        *)   info "удаление ядер для ${PKG_MGR:-этой системы} не реализовано" ;;
    esac
}

do_cleanup() {
    if ! is_root; then
        err "очистка требует root: запустите через sudo"
        exit 1
    fi

    local before after freed
    before=$(avail_bytes)

    if ! confirm "Выполнить очистку на $TARGET? Свободно сейчас: $(human "${before:-0}")"; then
        info "отменено"
        return 0
    fi

    clean_journal
    clean_pkg_cache
    clean_logs
    clean_tmp
    clean_coredumps
    clean_docker
    clean_docker_logs
    clean_kernels

    section "Результат"
    if (( DRY_RUN )); then
        info "режим --dry-run: ничего не изменено"
        return 0
    fi

    after=$(avail_bytes)
    freed=$(( ${after:-0} - ${before:-0} ))
    info "было свободно:  $(human "${before:-0}")"
    info "стало свободно: $(human "${after:-0}")"
    if (( freed > 0 )); then
        ok "освобождено: $(human "$freed")"
    else
        warn "место не освободилось — скорее всего, его держат процессы"
        info "см. раздел «Удалённые файлы, удерживаемые процессами» и перезапустите эти службы"
    fi
}

# ===========================================================================

main() {
    parse_args "$@"
    detect_pkg_mgr

    printf '%s%scleanup.sh %s%s — %s, %s\n' \
        "$C_BOLD" "$C_BLUE" "$VERSION" "$C_RESET" \
        "$(hostname 2>/dev/null || echo '?')" "$(date '+%F %T')"
    is_root || warn "запущено не от root: часть каталогов недоступна, отчёт будет неполным"

    analyze_filesystems
    analyze_top_dirs
    analyze_big_files
    analyze_held_files
    analyze_journal
    analyze_pkg_cache
    analyze_kernels
    analyze_docker
    analyze_logs
    analyze_tmp
    analyze_homes
    report_summary

    if (( DO_CLEAN )); then
        do_cleanup
    fi
    return 0
}

main "$@"
