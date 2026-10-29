#!/usr/bin/env bash
# ==============================================================================
# Rustyfin — Build & Install Script
# ==============================================================================
# Führt Quality Gates durch, baut WebUI und Backend, installiert das Binary
# und richtet den systemd-Service ein.
#
# Verwendung:
#   ./build.sh              # Vollständiger Build + Install (als Root)
#   ./build.sh --check      # Nur Quality Gates (fmt, clippy, test)
#   ./build.sh --web        # Nur WebUI bauen
#   ./build.sh --backend    # Nur Backend bauen (Release)
#   ./build.sh --install    # Binary + systemd-Service installieren (als Root)
#   ./build.sh --service    # Nur systemd-Service einrichten (als Root)
# ==============================================================================

set -euo pipefail

# ── Farben ────────────────────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

# ── Konfiguration ─────────────────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

SERVICE_NAME="rustyfin"
SERVICE_FILE="${SCRIPT_DIR}/${SERVICE_NAME}.service"
BINARY_NAME="rustyfin"
BINARY_SRC="${SCRIPT_DIR}/target/release/${BINARY_NAME}"
BINARY_DST="/usr/local/bin/${BINARY_NAME}"
DATA_DIR="/var/lib/${SERVICE_NAME}"
SERVICE_USER="${SERVICE_NAME}"
SERVICE_GROUP="${SERVICE_NAME}"
WEB_DIR="${SCRIPT_DIR}/crates/rustyfin-web"

# ── Logging ───────────────────────────────────────────────────────────────────
print_banner() {
    echo -e "${CYAN}${BOLD}"
    echo "  ╔═════════════════════════════════════════════════════════════════╗"
    echo "  ║                     R U S T Y F I N  B U I L D                 ║"
    echo "  ║           High-Performance Media Server in Rust                 ║"
    echo "  ╚═════════════════════════════════════════════════════════════════╝"
    echo -e "${NC}"
}

log_step() {
    echo -e "${BLUE}[STEP]${NC} ${BOLD}$1${NC}"
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

log_duration() {
    local duration=$1
    echo -e "  ${CYAN}⏱  Dauer: ${duration}s${NC}"
}

# ── Prerequisites ─────────────────────────────────────────────────────────────
check_prerequisites() {
    log_step "Prüfe Voraussetzungen..."

    local missing=0

    if ! command -v cargo &>/dev/null; then
        log_error "cargo (Rust) ist nicht installiert."
        missing=1
    else
        log_success "Rust: $(cargo --version | awk '{print $2}')"
    fi

    if ! command -v npm &>/dev/null; then
        log_error "npm (Node.js) ist nicht installiert."
        missing=1
    else
        log_success "Node.js: $(node --version), npm: $(npm --version)"
    fi

    if [ $missing -eq 1 ]; then
        log_error "Bitte installiere die fehlenden Abhängigkeiten."
        exit 1
    fi
}

# ── Quality Gates ─────────────────────────────────────────────────────────────
run_quality_gates() {
    log_step "Quality Gates — cargo fmt, clippy, test"

    # 1. Format check
    log_info "Prüfe Code-Formatierung (cargo fmt --check)..."
    local start=$SECONDS
    if cargo fmt --all -- --check; then
        log_success "Formatierung: OK ($(($SECONDS - start))s)"
    else
        log_error "Formatierung fehlerhaft — führe 'cargo fmt' aus."
        return 1
    fi

    # 2. Clippy
    log_info "Prüfe Clippy-Warnungen (cargo clippy --workspace -- -D warnings)..."
    start=$SECONDS
    if cargo clippy --workspace -- -D warnings 2>&1; then
        log_success "Clippy: Keine Warnungen ($(($SECONDS - start))s)"
    else
        log_error "Clippy-Warnungen gefunden — behebe sie vor dem Build."
        return 1
    fi

    # 3. Tests
    log_info "Führe Tests aus (cargo test --workspace)..."
    start=$SECONDS
    if cargo test --workspace 2>&1; then
        log_success "Tests: Alle bestanden ($(($SECONDS - start))s)"
    else
        log_error "Tests fehlgeschlagen — behebe sie vor dem Build."
        return 1
    fi

    log_success "Alle Quality Gates bestanden ✓"
}

# ── WebUI Build ───────────────────────────────────────────────────────────────
build_web() {
    log_step "Bau WebUI (Vite)..."

    if [ ! -d "$WEB_DIR" ]; then
        log_warn "WebUI-Verzeichnis nicht gefunden ($WEB_DIR) — Überspringen."
        return 0
    fi

    cd "$WEB_DIR"

    # npm install wenn nötig
    if [ ! -d "node_modules" ]; then
        log_info "Installiere Web-Abhängigkeiten (npm install)..."
        npm install --silent
        log_success "npm install abgeschlossen."
    fi

    # Vite build
    local start=$SECONDS
    log_info "Kompiliere Frontend (npm run build)..."
    if npm run build --silent 2>&1; then
        log_success "WebUI gebaut ($(($SECONDS - start))s)"
    else
        log_error "WebUI-Build fehlgeschlagen!"
        return 1
    fi

    # Verifiziere dist/
    if [ -f "dist/index.html" ]; then
        log_success "dist/index.html vorhanden."
    else
        log_error "dist/index.html nicht gefunden nach Build!"
        return 1
    fi

    cd "$SCRIPT_DIR"
}

# ── Backend Build ─────────────────────────────────────────────────────────────
build_backend() {
    log_step "Bau Backend (cargo build --release)..."

    local start=$SECONDS
    if cargo build --release --bin rustyfin 2>&1; then
        log_success "Backend Release-Build abgeschlossen ($(($SECONDS - start))s)"
    else
        log_error "Backend-Build fehlgeschlagen!"
        return 1
    fi

    if [ -f "$BINARY_SRC" ]; then
        local size
        size=$(du -h "$BINARY_SRC" | cut -f1)
        log_success "Binary: ${BINARY_SRC} (${size})"
    else
        log_error "Binary nicht gefunden: $BINARY_SRC"
        return 1
    fi
}

# ── Install Binary ────────────────────────────────────────────────────────────
install_binary() {
    log_step "Installiere Binary..."

    if [ ! -f "$BINARY_SRC" ]; then
        log_error "Binary nicht gefunden: $BINARY_SRC"
        log_info "Führe zuerst './build.sh --backend' aus."
        return 1
    fi

    # Binary kopieren
    cp "$BINARY_SRC" "$BINARY_DST"
    chmod 755 "$BINARY_DST"
    log_success "Binary installiert: $BINARY_DST"

    # Version anzeigen
    local version
    version=$("$BINARY_DST" --version 2>/dev/null || echo "unknown")
    log_info "Version: $version"
}

# ── Setup systemd Service ─────────────────────────────────────────────────────
setup_systemd() {
    log_step "Richte systemd-Service ein..."

    # Prüfe, ob als Root ausgeführt wird
    if [ "$(id -u)" -ne 0 ]; then
        log_error "systemd-Setup erfordert Root-Rechte. Führe mit sudo aus."
        return 1
    fi

    # Service-Datei prüfen
    if [ ! -f "$SERVICE_FILE" ]; then
        log_error "Service-Datei nicht gefunden: $SERVICE_FILE"
        return 1
    fi

    # Dedizierten Benutzer erstellen (falls nicht existiert)
    if ! getent group "$SERVICE_GROUP" &>/dev/null; then
        log_info "Erstelle Gruppe '${SERVICE_GROUP}'..."
        groupadd --system "$SERVICE_GROUP"
        log_success "Gruppe '${SERVICE_GROUP}' erstellt."
    else
        log_info "Gruppe '${SERVICE_GROUP}' existiert bereits."
    fi

    if ! getent passwd "$SERVICE_USER" &>/dev/null; then
        log_info "Erstelle System-User '${SERVICE_USER}'..."
        useradd --system --gid "$SERVICE_GROUP" --home-dir "$DATA_DIR" \
            --shell /usr/sbin/nologin --comment "Rustyfin Media Server" "$SERVICE_USER"
        log_success "User '${SERVICE_USER}' erstellt."
    else
        log_info "User '${SERVICE_USER}' existiert bereits."
    fi

    # Datenverzeichnis erstellen
    if [ ! -d "$DATA_DIR" ]; then
        log_info "Erstelle Datenverzeichnis '$DATA_DIR'..."
        mkdir -p "$DATA_DIR"
        log_success "Datenverzeichnis erstellt."
    fi

    chown "${SERVICE_USER}:${SERVICE_GROUP}" "$DATA_DIR"
    chmod 750 "$DATA_DIR"
    log_info "Berechtigungen gesetzt: $DATA_DIR (${SERVICE_USER}:${SERVICE_GROUP})"

    # Service-Datei installieren
    cp "$SERVICE_FILE" /etc/systemd/system/
    log_success "Service-Datei installiert: /etc/systemd/system/${SERVICE_NAME}.service"

    # systemd neu laden
    systemctl daemon-reload
    log_success "systemd daemon-reload abgeschlossen."

    # Service enable (autostart)
    systemctl enable "$SERVICE_NAME"
    log_success "Service '${SERVICE_NAME}' für Autostart aktiviert."

    echo ""
    log_info "═══════════════════════════════════════════════════════════════"
    log_info "  Installation abgeschlossen! Nächste Schritte:"
    echo ""
    log_info "  Service starten:"
    echo "    ${CYAN}sudo systemctl start ${SERVICE_NAME}${NC}"
    echo ""
    log_info "  Service-Status prüfen:"
    echo "    ${CYAN}systemctl status ${SERVICE_NAME}${NC}"
    echo ""
    log_info "  Logs anzeigen:"
    echo "    ${CYAN}journalctl -u ${SERVICE_NAME} -f${NC}"
    echo ""
    log_info "  Konfiguration ändern:"
    echo "    ${CYAN}sudo nano /etc/systemd/system/${SERVICE_NAME}.service${NC}"
    echo "    ${CYAN}sudo systemctl daemon-reload${NC}"
    echo ""
    log_info "  Datenverzeichnis:"
    echo "    ${CYAN}${DATA_DIR}${NC}"
    echo ""
    log_info "  Standard-Login: Benutzer ${BOLD}admin${NC} / Passwort ${BOLD}admin${NC}"
    log_info "  Web-Interface: http://localhost:8096/web/index.html"
    log_info "═══════════════════════════════════════════════════════════════"
}

# ── Full Build ────────────────────────────────────────────────────────────────
full_build() {
    print_banner

    local total_start=$SECONDS

    check_prerequisites
    echo ""

    run_quality_gates
    echo ""

    build_web
    echo ""

    build_backend
    echo ""

    local total_duration=$((SECONDS - total_start))
    echo ""
    log_success "═══════════════════════════════════════════════════════════════"
    log_success "  Build erfolgreich abgeschlossen! (${total_duration}s)"
    log_success "═══════════════════════════════════════════════════════════════"
    echo ""
    log_info "Für die Installation führe aus:"
    echo "  ${CYAN}sudo ./build.sh --install${NC}"
}

# ── Main ──────────────────────────────────────────────────────────────────────
main() {
    local mode="${1:---all}"

    case "$mode" in
        --check)
            print_banner
            check_prerequisites
            run_quality_gates
            ;;
        --web)
            print_banner
            build_web
            ;;
        --backend)
            print_banner
            build_backend
            ;;
        --install)
            print_banner
            if [ "$(id -u)" -ne 0 ]; then
                log_error "Install erfordert Root-Rechte. Führe mit sudo aus."
                exit 1
            fi
            install_binary
            echo ""
            setup_systemd
            ;;
        --service)
            print_banner
            setup_systemd
            ;;
        --all|"")
            full_build
            ;;
        --help|-h)
            echo "Verwendung:"
            echo "  ./build.sh              # Vollständiger Build (Quality Gates + WebUI + Backend)"
            echo "  ./build.sh --check      # Nur Quality Gates (fmt, clippy, test)"
            echo "  ./build.sh --web        # Nur WebUI bauen"
            echo "  ./build.sh --backend    # Nur Backend bauen (Release)"
            echo "  ./build.sh --install    # Binary + systemd-Service installieren (Root)"
            echo "  ./build.sh --service    # Nur systemd-Service einrichten (Root)"
            ;;
        *)
            log_error "Unbekannter Befehl: $mode"
            log_info "Verwende './build.sh --help' für Hilfe."
            exit 1
            ;;
    esac
}

main "$@"
