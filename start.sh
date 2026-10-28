#!/usr/bin/env bash
# ==============================================================================
# Rustyfin - Start & Test Script
# ==============================================================================
# Dieses Skript ermöglicht das einfache lokale Bauen, Starten, Stoppen und
# Testen des Rustyfin-Servers sowie der Web-Oberfläche.
# ==============================================================================

set -e

# Farben für formatierte Konsolenausgabe
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m' # No Color

# Verzeichnisse & Standardwerte
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

PORT="${PORT:-8096}"
HOST="${HOST:-127.0.0.1}"
BIND_ADDR="${RUSTYFIN_BIND:-${HOST}:${PORT}}"
DATA_DIR="${RUSTYFIN_DATA:-$SCRIPT_DIR/target/dev-data}"
PID_FILE="$SCRIPT_DIR/target/rustyfin.pid"
LOG_FILE="$SCRIPT_DIR/target/rustyfin.log"
BASE_URL="http://${HOST}:${PORT}"

print_banner() {
    echo -e "${CYAN}${BOLD}"
    echo "  ╔═════════════════════════════════════════════════════════════════╗"
    echo "  ║                       R U S T Y F I N                           ║"
    echo "  ║           High-Performance Media Server in Rust                 ║"
    echo "  ╚═════════════════════════════════════════════════════════════════╝"
    echo -e "${NC}"
}

log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[OK]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# ------------------------------------------------------------------------------
# Frontend / Web-Client Bauen
# ------------------------------------------------------------------------------
build_web() {
    log_info "Prüfe und erstelle Web-Client (crates/rustyfin-web)..."
    if [ -f "$SCRIPT_DIR/crates/rustyfin-web/package.json" ]; then
        if command -v npm >/dev/null 2>&1; then
            (
                cd "$SCRIPT_DIR/crates/rustyfin-web"
                if [ ! -d "node_modules" ]; then
                    log_info "Installiere Web-Abhängigkeiten (npm install)..."
                    npm install --silent
                fi
                log_info "Kompiliere Frontend-Assets (npm run build)..."
                npm run build --silent
            )
            log_success "Web-Client erfolgreich gebaut (dist/ vorhanden)."
        else
            if [ -f "$SCRIPT_DIR/crates/rustyfin-web/dist/index.html" ]; then
                log_warn "npm nicht gefunden, verwende vorhandenes dist/-Verzeichnis."
            else
                log_error "npm ist nicht installiert und kein dist/-Verzeichnis vorhanden!"
                return 1
            fi
        fi
    fi
}

# ------------------------------------------------------------------------------
# Backend & Workspace Bauen
# ------------------------------------------------------------------------------
build_backend() {
    local release_flag=""
    if [ "$1" == "--release" ]; then
        release_flag="--release"
        log_info "Kompiliere Rustyfin im Release-Modus..."
    else
        log_info "Kompiliere Rustyfin im Debug-Modus..."
    fi

    cargo build --workspace $release_flag
    log_success "Rust-Workspace erfolgreich kompiliert."
}

# ------------------------------------------------------------------------------
# Server im Vordergrund starten
# ------------------------------------------------------------------------------
run_foreground() {
    print_banner
    mkdir -p "$DATA_DIR"
    mkdir -p "$SCRIPT_DIR/target"

    # Prüfen, ob Port bereits belegt ist
    if command -v nc >/dev/null 2>&1; then
        if nc -z -w 1 "$HOST" "$PORT" 2>/dev/null; then
            log_warn "Port $PORT scheint bereits belegt zu sein. Eventuell läuft bereits eine Instanz."
        fi
    fi

    # Webclient prüfen
    if [ ! -f "$SCRIPT_DIR/crates/rustyfin-web/dist/index.html" ]; then
        build_web
    fi

    echo -e "${BOLD}Server-Konfiguration:${NC}"
    echo -e "  • Web-Client URL:   ${CYAN}${BOLD}${BASE_URL}/web/index.html${NC}"
    echo -e "  • API Base URL:     ${CYAN}${BASE_URL}/${NC}"
    echo -e "  • Standard-Login:   Benutzer: ${BOLD}admin${NC} | Passwort: ${BOLD}admin${NC}"
    echo -e "  • Datenverzeichnis: ${DATA_DIR}"
    echo -e "  • Bind-Adresse:     ${BIND_ADDR}"
    echo ""
    log_info "Starte Rustyfin Server (Strg+C zum Beenden)..."
    echo ""

    export RUSTYFIN_BIND="$BIND_ADDR"
    export RUSTYFIN_DATA="$DATA_DIR"
    export RUST_LOG="${RUST_LOG:-info,rustyfin_server=debug,rustyfin_api=debug}"
    export RUSTYFIN_WEBCLIENT_DIR="$SCRIPT_DIR/crates/rustyfin-web/dist"

    cargo run --bin rustyfin
}

# ------------------------------------------------------------------------------
# Server im Hintergrund starten (Daemon)
# ------------------------------------------------------------------------------
start_background() {
    print_banner
    mkdir -p "$DATA_DIR"
    mkdir -p "$SCRIPT_DIR/target"

    if [ -f "$PID_FILE" ]; then
        PID=$(cat "$PID_FILE")
        if kill -0 "$PID" 2>/dev/null; then
            log_warn "Rustyfin läuft bereits im Hintergrund mit PID $PID."
            echo -e "  Web-Client: ${CYAN}${BASE_URL}/web/index.html${NC}"
            return 0
        fi
    fi

    if [ ! -f "$SCRIPT_DIR/crates/rustyfin-web/dist/index.html" ]; then
        build_web
    fi

    log_info "Kompiliere Server falls nötig..."
    cargo build --bin rustyfin

    export RUSTYFIN_BIND="$BIND_ADDR"
    export RUSTYFIN_DATA="$DATA_DIR"
    export RUST_LOG="${RUST_LOG:-info,rustyfin_server=debug,rustyfin_api=debug}"
    export RUSTYFIN_WEBCLIENT_DIR="$SCRIPT_DIR/crates/rustyfin-web/dist"

    log_info "Starte Rustyfin im Hintergrund..."
    "$SCRIPT_DIR/target/debug/rustyfin" > "$LOG_FILE" 2>&1 &
    PID=$!
    echo "$PID" > "$PID_FILE"

    log_info "Warte auf Initialisierung (PID $PID)..."
    local count=0
    local max=30
    local is_up=0

    while [ $count -lt $max ]; do
        sleep 0.5
        if curl -s -f "${BASE_URL}/System/Ping" >/dev/null 2>&1 || curl -s -f "${BASE_URL}/health" >/dev/null 2>&1; then
            is_up=1
            break
        fi
        if ! kill -0 "$PID" 2>/dev/null; then
            log_error "Server-Prozess unerwartet beendet. Siehe Log-Datei: $LOG_FILE"
            rm -f "$PID_FILE"
            return 1
        fi
        count=$((count + 1))
    done

    if [ $is_up -eq 1 ]; then
        log_success "Rustyfin Server erfolgreich gestartet!"
        echo -e "  • Web-Client:       ${CYAN}${BOLD}${BASE_URL}/web/index.html${NC}"
        echo -e "  • Standard-Login:   Benutzer: ${BOLD}admin${NC} | Passwort: ${BOLD}admin${NC}"
        echo -e "  • Logs:             tail -f $LOG_FILE"
        echo -e "  • Stoppen mit:      ./start.sh stop"
    else
        log_warn "Server gestartet (PID $PID), antwortet aber noch nicht auf Ping. Prüfe Logs: $LOG_FILE"
    fi
}

# ------------------------------------------------------------------------------
# Server stoppen
# ------------------------------------------------------------------------------
stop_server() {
    if [ -f "$PID_FILE" ]; then
        PID=$(cat "$PID_FILE")
        if kill -0 "$PID" 2>/dev/null; then
            log_info "Stoppe Rustyfin Server (PID $PID)..."
            kill "$PID" 2>/dev/null || true
            local count=0
            while kill -0 "$PID" 2>/dev/null && [ $count -lt 10 ]; do
                sleep 0.5
                count=$((count + 1))
            done
            if kill -0 "$PID" 2>/dev/null; then
                kill -9 "$PID" 2>/dev/null || true
            fi
            log_success "Server gestoppt."
        else
            log_warn "Prozess $PID läuft nicht mehr."
        fi
        rm -f "$PID_FILE"
    else
        log_info "Keine aktive PID-Datei ($PID_FILE) gefunden."
        # Versuch über pkill
        pkill -f "target/.*/rustyfin" 2>/dev/null && log_success "Laufende Server-Prozesse beendet." || true
    fi
}

# ------------------------------------------------------------------------------
# Status überprüfen
# ------------------------------------------------------------------------------
status_server() {
    local running=0
    if [ -f "$PID_FILE" ]; then
        PID=$(cat "$PID_FILE")
        if kill -0 "$PID" 2>/dev/null; then
            running=1
            echo -e "Status: ${GREEN}${BOLD}LÄUFT${NC} (PID: $PID)"
        fi
    fi

    if [ $running -eq 0 ]; then
        echo -e "Status: ${RED}NICHT AKTIV${NC}"
    fi

    echo -n "HTTP-Endpunkt (${BASE_URL}): "
    if curl -s -f -o /dev/null -w "%{http_code}" "${BASE_URL}/health" 2>/dev/null | grep -q "200"; then
        echo -e "${GREEN}ERREICHBAR (/health -> OK)${NC}"
    elif curl -s -f -o /dev/null -w "%{http_code}" "${BASE_URL}/System/Ping" 2>/dev/null | grep -q "200"; then
        echo -e "${GREEN}ERREICHBAR (/System/Ping -> OK)${NC}"
    else
        echo -e "${YELLOW}Keine Antwort${NC}"
    fi

    echo -n "Web-Interface (${BASE_URL}/web/index.html): "
    local web_status
    web_status=$(curl -s -o /dev/null -w "%{http_code}" "${BASE_URL}/web/index.html" 2>/dev/null || true)
    if [ "$web_status" == "200" ]; then
        echo -e "${GREEN}OK (200)${NC}"
    elif [ "$web_status" == "000" ] || [ -z "$web_status" ]; then
        echo -e "${YELLOW}Keine Antwort${NC}"
    else
        echo -e "${YELLOW}Status $web_status${NC}"
    fi
}

# ------------------------------------------------------------------------------
# Tests ausführen
# ------------------------------------------------------------------------------
run_tests() {
    print_banner
    log_info "Führe alle Unit- und Integrationstests aus (cargo test --workspace)..."
    cargo test --workspace

    log_success "Alle Rust-Tests erfolgreich abgeschlossen!"
}

# ------------------------------------------------------------------------------
# Smoke-Test / Live-Endpunkt-Test
# ------------------------------------------------------------------------------
run_smoke_test() {
    print_banner
    log_info "Führe Live-System- und HTTP-Smoke-Tests durch..."

    local was_running=1
    if [ ! -f "$PID_FILE" ] || ! kill -0 "$(cat "$PID_FILE" 2>/dev/null)" 2>/dev/null; then
        was_running=0
        log_info "Starte temporäre Server-Instanz für Smoke-Tests..."
        start_background
    fi

    echo ""
    log_info "Prüfe Endpunkte an $BASE_URL ..."

    local failed=0

    # 1. Health
    echo -n "  • GET /health : "
    local code
    code=$(curl -s -o /dev/null -w "%{http_code}" "$BASE_URL/health")
    if [ "$code" == "200" ]; then
        echo -e "${GREEN}200 OK${NC}"
    else
        echo -e "${RED}Fehler ($code)${NC}"; failed=$((failed + 1))
    fi

    # 2. Ping
    echo -n "  • GET /System/Ping : "
    code=$(curl -s -o /dev/null -w "%{http_code}" "$BASE_URL/System/Ping")
    if [ "$code" == "200" ]; then
        echo -e "${GREEN}200 OK${NC}"
    else
        echo -e "${RED}Fehler ($code)${NC}"; failed=$((failed + 1))
    fi

    # 3. Public System Info
    echo -n "  • GET /System/Info/Public : "
    local info_resp
    info_resp=$(curl -s "$BASE_URL/System/Info/Public")
    if echo "$info_resp" | grep -q "ServerName"; then
        echo -e "${GREEN}200 OK (Gültige JSON-Antwort)${NC}"
    else
        echo -e "${RED}Ungültige Antwort: $info_resp${NC}"; failed=$((failed + 1))
    fi

    # 4. Root Redirect zu /web/index.html
    echo -n "  • GET / (Redirect zu Webclient) : "
    local redirect_loc
    redirect_loc=$(curl -s -I "$BASE_URL/" | grep -i "^location:" | tr -d '\r\n' | awk '{print $2}')
    if [[ "$redirect_loc" == *"/web/index.html"* ]]; then
        echo -e "${GREEN}OK (Redirect -> $redirect_loc)${NC}"
    else
        echo -e "${YELLOW}Hinweis: Redirect-Header war '$redirect_loc'${NC}"
    fi

    # 5. Web Client HTML
    echo -n "  • GET /web/index.html : "
    local web_body
    web_body=$(curl -s "$BASE_URL/web/index.html")
    if echo "$web_body" | grep -iq "Rustyfin"; then
        echo -e "${GREEN}200 OK (Rustyfin Webclient geladen)${NC}"
    else
        echo -e "${RED}Webclient-Inhalt nicht gefunden!${NC}"; failed=$((failed + 1))
    fi

    # 6. Web Assets
    echo -n "  • GET /web/assets/app.js : "
    code=$(curl -s -o /dev/null -w "%{http_code}" "$BASE_URL/web/assets/app.js")
    if [ "$code" == "200" ]; then
        echo -e "${GREEN}200 OK${NC}"
    else
        echo -e "${RED}Fehler ($code)${NC}"; failed=$((failed + 1))
    fi

    # 7. Authentifizierung Test
    echo -n "  • POST /Users/AuthenticateByName (admin / admin) : "
    local auth_resp
    auth_resp=$(curl -s -X POST "$BASE_URL/Users/AuthenticateByName" \
        -H "Content-Type: application/json" \
        -d '{"Username":"admin","Pw":"admin"}')
    if echo "$auth_resp" | grep -q "AccessToken"; then
        echo -e "${GREEN}200 OK (Authentifizierung erfolgreich)${NC}"
    else
        echo -e "${RED}Authentifizierung fehlgeschlagen: $auth_resp${NC}"; failed=$((failed + 1))
    fi

    echo ""
    if [ $was_running -eq 0 ]; then
        log_info "Beende temporäre Server-Instanz..."
        stop_server
    fi

    if [ $failed -eq 0 ]; then
        log_success "Alle Smoke-Tests erfolgreich bestanden!"
    else
        log_error "$failed Smoke-Test(s) fehlgeschlagen!"
        return 1
    fi
}

# ------------------------------------------------------------------------------
# Bereinigen & Frischer Start
# ------------------------------------------------------------------------------
clean_env() {
    log_info "Bereinige temporäre Testdaten und PID-Dateien..."
    stop_server || true
    rm -rf "$DATA_DIR"
    rm -f "$PID_FILE" "$LOG_FILE"
    log_success "Bereinigung abgeschlossen."
}

init_fresh() {
    log_info "Initialisiere sauberen Neustart (--fresh)..."
    stop_server >/dev/null 2>&1 || true
    rm -rf "$DATA_DIR"
    rm -f "$PID_FILE" "$LOG_FILE"
    mkdir -p "$DATA_DIR"
    log_success "Datenverzeichnis ($DATA_DIR) und temporäre Dateien vollständig zurückgesetzt."
}

# ------------------------------------------------------------------------------
# Live-Logs anzeigen
# ------------------------------------------------------------------------------
show_logs() {
    local lines=100
    for opt in "$@"; do
        if [[ "$opt" =~ ^[0-9]+$ ]]; then
            lines="$opt"
        fi
    done

    mkdir -p "$SCRIPT_DIR/target"

    if [ ! -f "$LOG_FILE" ]; then
        log_warn "Log-Datei ($LOG_FILE) existiert noch nicht. Erstelle leere Log-Datei..."
        touch "$LOG_FILE"
    fi

    if [ -f "$PID_FILE" ] && kill -0 "$(cat "$PID_FILE" 2>/dev/null)" 2>/dev/null; then
        local pid
        pid=$(cat "$PID_FILE")
        log_info "Zeige Live-Logs für Rustyfin Server (PID: $pid, $LOG_FILE)... (Strg+C zum Beenden)"
    else
        log_warn "Rustyfin Server läuft derzeit nicht im Hintergrund. Zeige bisherige Log-Einträge ($LOG_FILE)..."
    fi

    echo -e "${CYAN}─────────────────────────────────────────────────────────────────────────────${NC}"
    tail -n "$lines" -f "$LOG_FILE"
}

# ------------------------------------------------------------------------------
# Browser öffnen
# ------------------------------------------------------------------------------
open_browser() {
    local url="${BASE_URL}/web/index.html"
    log_info "Öffne Web-Client in deinem Standard-Browser ($url)..."
    if command -v xdg-open >/dev/null 2>&1; then
        xdg-open "$url" >/dev/null 2>&1 &
    elif command -v open >/dev/null 2>&1; then
        open "$url" >/dev/null 2>&1 &
    else
        echo -e "Bitte öffne die folgende URL in deinem Browser: ${CYAN}${BOLD}$url${NC}"
    fi
}

# ------------------------------------------------------------------------------
# Hilfe anzeigen
# ------------------------------------------------------------------------------
show_help() {
    print_banner
    echo -e "${BOLD}Verwendung:${NC} ./start.sh [BEFEHL] [OPTIONEN]"
    echo ""
    echo -e "${BOLD}Befehle:${NC}"
    echo -e "  ${GREEN}run${NC} (Standard)    Startet den Server im Vordergrund (Strg+C zum Beenden)"
    echo -e "  ${GREEN}fresh, reset${NC}   Initialisiert einen sauberen Neustart (Datenbank & Cache zurücksetzen)"
    echo -e "  ${GREEN}start${NC}          Startet den Server im Hintergrund (Daemon-Modus)"
    echo -e "  ${GREEN}logs, log${NC}      Zeigt die Live-Logs des Hintergrund-Servers in Echtzeit an (tail -f)"
    echo -e "  ${GREEN}stop${NC}           Stoppt den im Hintergrund laufenden Server"
    echo -e "  ${GREEN}restart${NC}        Startet den Hintergrund-Server neu"
    echo -e "  ${GREEN}status${NC}         Zeigt den aktuellen Status und prüft die Erreichbarkeit"
    echo -e "  ${GREEN}test${NC}           Führt alle Unit- und Integrationstests (cargo test) aus"
    echo -e "  ${GREEN}smoke${NC}          Führt Live-HTTP-Smoke-Tests gegen alle Haupt-Endpunkte durch"
    echo -e "  ${GREEN}build${NC}          Baut den Web-Client und den Rust-Workspace"
    echo -e "  ${GREEN}web${NC}            Baut ausschließlich das Web-Frontend (npm run build)"
    echo -e "  ${GREEN}open${NC}           Öffnet die Web-Oberfläche im Browser"
    echo -e "  ${GREEN}clean${NC}          Stoppt den Server und löscht temporäre Entwicklungsdaten"
    echo -e "  ${GREEN}help, -h${NC}       Zeigt diese Hilfeseite an"
    echo ""
    echo -e "${BOLD}Optionen & Schalter:${NC}"
    echo -e "  ${CYAN}--fresh, -f${NC}    Setzt Datenbank, Cache und Session-Dateien vor dem Start zurück"
    echo -e "  ${CYAN}--release, -r${NC}  Führt Build oder Start im optimierten Release-Modus aus"
    echo ""
    echo -e "${BOLD}Umgebungsvariablen:${NC}"
    echo -e "  PORT             Port des Servers (Standard: 8096)"
    echo -e "  HOST             Host-Adresse (Standard: 127.0.0.1)"
    echo -e "  RUST_LOG         Log-Filter (z.B. debug, info, warn)"
    echo -e "  RUSTYFIN_DATA    Speicherpfad für Datenbank und Medienkonfiguration"
    echo ""
    echo -e "${BOLD}Beispiele:${NC}"
    echo -e "  ./start.sh                  # Direkt im Terminal starten (inkl. Live-Logs)"
    echo -e "  ./start.sh start            # Im Hintergrund starten"
    echo -e "  ./start.sh logs             # Live-Logs des Hintergrund-Servers verfolgen"
    echo -e "  ./start.sh logs 50          # Die letzten 50 Zeilen anzeigen und live verfolgen"
    echo -e "  ./start.sh --fresh          # Frischer Start (Datenbank & Cache zurückgesetzt)"
    echo -e "  ./start.sh fresh            # Kurzform für frischen Start im Vordergrund"
    echo -e "  ./start.sh start --fresh    # Frischer Start im Hintergrund (Daemon)"
    echo -e "  ./start.sh test             # Tests ausführen"
    echo -e "  ./start.sh smoke            # Schneller Live-Endpunkt-Test"
    echo -e "  PORT=9000 ./start.sh        # Auf benutzerdefiniertem Port starten"
    echo ""
}

# ------------------------------------------------------------------------------
# Befehls- & Options-Routing
# ------------------------------------------------------------------------------
COMMAND=""
FRESH=0
RELEASE=""
EXTRA_ARGS=()

for arg in "$@"; do
    case "$arg" in
        --fresh|-f)
            FRESH=1
            ;;
        --release|-r)
            RELEASE="--release"
            ;;
        fresh|fresh-start|reset)
            FRESH=1
            if [ -z "$COMMAND" ]; then
                COMMAND="run"
            fi
            ;;
        run|start|stop|restart|logs|log|tail|follow|status|test|smoke|check|smoke-test|build|web|open|clean|help|-h|--help)
            if [ -z "$COMMAND" ]; then
                COMMAND="$arg"
            else
                EXTRA_ARGS+=("$arg")
            fi
            ;;
        *)
            if [ -z "$COMMAND" ]; then
                COMMAND="$arg"
            else
                EXTRA_ARGS+=("$arg")
            fi
            ;;
    esac
done

COMMAND="${COMMAND:-run}"

# Bei gesetztem --fresh Schalter Daten bereinigen
if [ "$FRESH" -eq 1 ] && [ "$COMMAND" != "clean" ] && [ "$COMMAND" != "stop" ] && [ "$COMMAND" != "help" ] && [ "$COMMAND" != "-h" ] && [ "$COMMAND" != "--help" ]; then
    init_fresh
fi

case "$COMMAND" in
    run)
        run_foreground
        ;;
    start)
        start_background
        ;;
    logs|log|tail|follow)
        show_logs "${EXTRA_ARGS[@]}"
        ;;
    stop)
        stop_server
        ;;
    restart)
        stop_server
        sleep 1
        start_background
        ;;
    status)
        status_server
        ;;
    test)
        run_tests
        ;;
    smoke|check|smoke-test)
        run_smoke_test
        ;;
    build)
        build_web
        build_backend "${RELEASE:-${EXTRA_ARGS[0]:-}}"
        ;;
    web)
        build_web
        ;;
    open)
        open_browser
        ;;
    clean)
        clean_env
        ;;
    help|-h|--help)
        show_help
        ;;
    *)
        log_error "Unbekannter Befehl: '$COMMAND'"
        echo ""
        show_help
        exit 1
        ;;
esac
