#!/bin/bash
set -euo pipefail

# E-invoice validation using KoSIT validator.
# Generates sample e-invoices and validates them against XRechnung/Factur-X schemas.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(dirname "$SCRIPT_DIR")"

# Use provided KOSIT_DIR or default to target/kosit
KOSIT_DIR="${KOSIT_DIR:-${REPO_ROOT}/target/kosit}"
VALIDATOR_JAR="${KOSIT_DIR}/validator-1.6.3-standalone.jar"
XRECHNUNG_ZIP="${KOSIT_DIR}/xrechnung-3.0.2-validator-configuration-2026-08-31.zip"
XRECHNUNG_CONFIG_DIR="${KOSIT_DIR}/xrechnung-3.0.2-configuration"

# Sample output directory
SAMPLES_DIR="${KOSIT_DIR}/samples"
REPORTS_DIR="${KOSIT_DIR}/reports"

# Expected checksums
VALIDATOR_SHA256="799e64befca97d4080e03608c80b85dd5a5ecc5f4ae4f35d1116ec2855b9a7c9"
XRECHNUNG_SHA256="2530cd107c414511c5d0462ec10f886910395abfca820db82e83d70bf01221a8"

# Create directories
mkdir -p "$KOSIT_DIR" "$SAMPLES_DIR" "$REPORTS_DIR"

echo "E-invoice validation using KoSIT validator"
echo "==========================================="

# Download validator JAR if not present
if [[ ! -f "$VALIDATOR_JAR" ]]; then
    echo "Downloading KoSIT validator 1.6.3..."
    curl -sL "https://github.com/itplr-kosit/validator/releases/download/v1.6.3/validator-1.6.3-standalone.jar" \
        -o "$VALIDATOR_JAR.tmp"

    # Verify checksum
    echo "$VALIDATOR_SHA256  $VALIDATOR_JAR.tmp" | sha256sum -c - || {
        echo "ERROR: Validator checksum mismatch"
        rm -f "$VALIDATOR_JAR.tmp"
        exit 1
    }
    mv "$VALIDATOR_JAR.tmp" "$VALIDATOR_JAR"
    echo "✓ Validator downloaded and verified"
else
    echo "✓ Validator already present"
fi

# Download XRechnung configuration if not present
if [[ ! -d "$XRECHNUNG_CONFIG_DIR" ]]; then
    echo "Downloading XRechnung configuration..."
    curl -sL "https://github.com/itplr-kosit/validator-configuration-xrechnung/releases/download/v2026-08-31/xrechnung-3.0.2-validator-configuration-2026-08-31.zip" \
        -o "$XRECHNUNG_ZIP.tmp"

    # Verify checksum
    echo "$XRECHNUNG_SHA256  $XRECHNUNG_ZIP.tmp" | sha256sum -c - || {
        echo "ERROR: XRechnung configuration checksum mismatch"
        rm -f "$XRECHNUNG_ZIP.tmp"
        exit 1
    }

    # Extract
    unzip -q "$XRECHNUNG_ZIP.tmp" -d "$KOSIT_DIR"
    rm -f "$XRECHNUNG_ZIP.tmp"
    echo "✓ XRechnung configuration downloaded and verified"
else
    echo "✓ XRechnung configuration already present"
fi

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

# Validate each sample
echo ""
echo "Validating samples with KoSIT validator..."
ACCEPTED=0
REJECTED=0
WARNINGS=0

for xml_file in "$SAMPLES_DIR"/*.xml; do
    filename=$(basename "$xml_file")
    report_file="$REPORTS_DIR/${filename%.xml}-report.xml"

    echo -n "  Validating $filename ... "

    # Run validator with -r (repository/config dir) and -o (output report dir)
    java -jar "$VALIDATOR_JAR" \
        -s "$XRECHNUNG_CONFIG_DIR/scenarios.xml" \
        -r "$XRECHNUNG_CONFIG_DIR" \
        -o "$REPORTS_DIR" \
        "$xml_file" \
        > /dev/null 2>&1

    exit_code=$?

    # Check report for acceptance status
    if [[ -f "$report_file" ]] && grep -q "<rep:accept>true</rep:accept>" "$report_file"; then
        echo "✓ ACCEPTED"
        ((ACCEPTED++))

        # Check for warnings
        if grep -q "<rep:warning" "$report_file"; then
            ((WARNINGS++))
            echo "    ⚠ Contains warnings (see report for details)"
        fi
    else
        echo "✗ REJECTED"
        ((REJECTED++))

        # Print rejection details if report exists
        if [[ -f "$report_file" ]]; then
            if grep -q "<rep:accept>false</rep:accept>" "$report_file"; then
                echo "    Rejection reason:"
                grep -A 5 "<rep:accept>false</rep:accept>" "$report_file" | head -10 | sed 's/^/      /'
            fi
        else
            echo "    (validator exit code: $exit_code, no report generated)"
        fi
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
