#!/bin/bash
set -euo pipefail

# E-invoice validation using KoSIT validator.
# Generates sample e-invoices and validates them against XRechnung/Factur-X schemas.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(dirname "$SCRIPT_DIR")"

# The validator files live outside target/. Swatinem/rust-cache prunes every regular
# file under target/ when it saves the cache, so a restored target/ kept the
# configuration directories but not their files. That made the validator fail
# with its usage text (exit 254) on every re-run (CI run 40117675036, attempt 2).
# Override with KOSIT_DIR if needed.
KOSIT_DIR="${KOSIT_DIR:-${REPO_ROOT}/.kosit}"
VALIDATOR_JAR="${KOSIT_DIR}/validator-1.6.3-standalone.jar"
XRECHNUNG_ZIP="${KOSIT_DIR}/xrechnung-3.0.2-validator-configuration-2026-08-31.zip"
XRECHNUNG_CONFIG_DIR="${KOSIT_DIR}/xrechnung-3.0.2-configuration"
# Written after a successful extraction; it records which zip the directory came from.
XRECHNUNG_CONFIG_STAMP="${KOSIT_DIR}/xrechnung-3.0.2-configuration.zip-sha256"

# Sample output directory
SAMPLES_DIR="${KOSIT_DIR}/samples"
REPORTS_DIR="${KOSIT_DIR}/reports"

# Expected checksums
VALIDATOR_SHA256="799e64befca97d4080e03608c80b85dd5a5ecc5f4ae4f35d1116ec2855b9a7c9"
XRECHNUNG_SHA256="2530cd107c414511c5d0462ec10f886910395abfca820db82e83d70bf01221a8"

# Create directories
mkdir -p "$KOSIT_DIR"

echo "E-invoice validation using KoSIT validator"
echo "==========================================="

# Check the jar on every run, not only after a download, so a damaged or
# partially restored file is replaced instead of being run.
if echo "$VALIDATOR_SHA256  $VALIDATOR_JAR" | sha256sum -c --status - 2>/dev/null; then
    echo "✓ Validator present and checksum verified"
else
    echo "Downloading KoSIT validator 1.6.3..."
    curl -fsSL "https://github.com/itplr-kosit/validator/releases/download/v1.6.3/validator-1.6.3-standalone.jar" \
        -o "$VALIDATOR_JAR.tmp"

    # Verify checksum
    echo "$VALIDATOR_SHA256  $VALIDATOR_JAR.tmp" | sha256sum -c - || {
        echo "ERROR: Validator checksum mismatch"
        rm -f "$VALIDATOR_JAR.tmp"
        exit 1
    }
    mv "$VALIDATOR_JAR.tmp" "$VALIDATOR_JAR"
    echo "✓ Validator downloaded and verified"
fi

# The configuration is trusted only when the stamp matches the pinned zip checksum
# and the entry file exists. A directory that merely exists is not enough.
xrechnung_config_ready() {
    [[ -f "$XRECHNUNG_CONFIG_DIR/scenarios.xml" ]] \
        && [[ -f "$XRECHNUNG_CONFIG_STAMP" ]] \
        && [[ "$(cat "$XRECHNUNG_CONFIG_STAMP")" == "$XRECHNUNG_SHA256" ]]
}

if xrechnung_config_ready; then
    echo "✓ XRechnung configuration present and verified"
else
    echo "Downloading XRechnung configuration..."
    rm -rf "$XRECHNUNG_CONFIG_DIR" "$XRECHNUNG_CONFIG_STAMP"
    curl -fsSL "https://github.com/itplr-kosit/validator-configuration-xrechnung/releases/download/v2026-08-31/xrechnung-3.0.2-validator-configuration-2026-08-31.zip" \
        -o "$XRECHNUNG_ZIP.tmp"

    # Verify checksum
    echo "$XRECHNUNG_SHA256  $XRECHNUNG_ZIP.tmp" | sha256sum -c - || {
        echo "ERROR: XRechnung configuration checksum mismatch"
        rm -f "$XRECHNUNG_ZIP.tmp"
        exit 1
    }

    # Extract
    mkdir -p "$XRECHNUNG_CONFIG_DIR"
    unzip -q "$XRECHNUNG_ZIP.tmp" -d "$XRECHNUNG_CONFIG_DIR"
    rm -f "$XRECHNUNG_ZIP.tmp"
    printf '%s\n' "$XRECHNUNG_SHA256" > "$XRECHNUNG_CONFIG_STAMP"
    xrechnung_config_ready || {
        echo "ERROR: XRechnung configuration extracted without scenarios.xml"
        exit 1
    }
    echo "✓ XRechnung configuration downloaded and verified"
fi

# Start from empty output directories so stale samples or reports from an
# earlier run cannot be counted or validated.
rm -rf "$SAMPLES_DIR" "$REPORTS_DIR"
mkdir -p "$SAMPLES_DIR" "$REPORTS_DIR"

# Generate sample e-invoices
echo ""
echo "Generating sample e-invoices..."
cd "$REPO_ROOT"
LEDGERS_EINVOICE_OUT="$SAMPLES_DIR" cargo test -p ledgers-invoice --test einvoice \
    --locked 2>&1 | grep -E "^test |^running |^result:"
echo "✓ Samples generated in $SAMPLES_DIR"

# Count samples
SAMPLE_COUNT=$(find "$SAMPLES_DIR" -name "*.xml" -type f | wc -l)
echo "  Generated $SAMPLE_COUNT sample files"

if [[ $SAMPLE_COUNT -eq 0 ]]; then
    echo "ERROR: No samples were generated"
    exit 1
fi

# Print every error-level message in a KoSIT report: rule ID (code attribute) and text.
# The report marks each finding as <rep:message level="error" code="BR-...">.
print_rejection_reasons() {
    local report="$1"
    local count i code text
    if command -v xmllint >/dev/null 2>&1; then
        count=$(xmllint --xpath "count(//*[local-name()='message'][@level='error' or @level='fatal'])" "$report" 2>/dev/null || echo 0)
        for ((i = 1; i <= count; i++)); do
            code=$(xmllint --xpath "string((//*[local-name()='message'][@level='error' or @level='fatal'])[$i]/@code)" "$report" 2>/dev/null || true)
            text=$(xmllint --xpath "string((//*[local-name()='message'][@level='error' or @level='fatal'])[$i])" "$report" 2>/dev/null | tr -s ' \n\t' ' ' || true)
            echo "      ${code:-<no rule id>}: ${text:0:240}"
        done
    else
        grep -o 'level="\(error\|fatal\)"[^>]*>[^<]\{1,240\}' "$report" | sed 's/^/      /' || true
    fi
}

# Validate each sample
echo ""
echo "Validating samples with KoSIT validator..."
ACCEPTED=0
REJECTED=0
WARNINGS=0

for xml_file in "$SAMPLES_DIR"/*.xml; do
    filename=$(basename "$xml_file")
    report_file="$REPORTS_DIR/${filename%.xml}-report.xml"
    log_file="$REPORTS_DIR/${filename%.xml}-validator.log"

    echo -n "  Validating $filename ... "

    # -s scenarios, -r repository (the XRechnung configuration), -o report output.
    # Console output goes to a log file so a failure is diagnosable from CI.
    set +e
    java -jar "$VALIDATOR_JAR" \
        -s "$XRECHNUNG_CONFIG_DIR/scenarios.xml" \
        -r "$XRECHNUNG_CONFIG_DIR" \
        -o "$REPORTS_DIR" \
        "$xml_file" \
        > "$log_file" 2>&1
    exit_code=$?
    set -e

    # The report states the verdict as an element: <rep:accept>, <rep:acceptable> or <rep:reject>.
    if [[ -f "$report_file" ]] && grep -q "<rep:reject>" "$report_file"; then
        echo "✗ REJECTED"
        REJECTED=$((REJECTED + 1))
        print_rejection_reasons "$report_file"
    elif [[ -f "$report_file" ]] && grep -Eq "<rep:accept(able)?>" "$report_file"; then
        echo "✓ ACCEPTED"
        ACCEPTED=$((ACCEPTED + 1))
        if grep -q '<rep:message [^>]*level="warning"' "$report_file"; then
            WARNINGS=$((WARNINGS + 1))
            echo "    ⚠ Contains warnings (see report for details)"
        fi
    else
        echo "✗ REJECTED"
        REJECTED=$((REJECTED + 1))
        echo "    (validator exit code: $exit_code, no verdict in report; the validator did not run the sample)"
        echo "    Validator error lines from $log_file:"
        { grep -m5 -E "Exception|Error|No test targets|Not a valid path" "$log_file" || tail -5 "$log_file"; } | sed 's/^/      /'
    fi
done

# Summary
echo ""
echo "Validation Summary"
echo "=================="
echo "Total samples: $SAMPLE_COUNT"
echo "Accepted: $ACCEPTED"
echo "Rejected: $REJECTED"
echo "With warnings: $WARNINGS"
echo ""
echo "Reports saved to: $REPORTS_DIR"

# Exit with failure if any were rejected
if [[ $REJECTED -gt 0 ]]; then
    echo "✗ VALIDATION FAILED: $REJECTED sample(s) were rejected"
    exit 1
fi

if [[ $ACCEPTED -ne $SAMPLE_COUNT ]]; then
    echo "✗ VALIDATION FAILED: Not all samples were accepted"
    exit 1
fi

if [[ $WARNINGS -gt 0 ]]; then
    echo "⚠ VALIDATION PASSED with warnings"
    exit 0
fi

echo "✓ VALIDATION PASSED: All samples accepted with no errors"
exit 0
