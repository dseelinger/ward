#!/usr/bin/env sh
#
# Ward — coverage floors, measured.
#
# Separate from check.sh because this one is slow: it builds the whole crate
# instrumented and runs every test. check.sh is what you run before every push
# and has to stay quick; this runs in CI.
#
# The floors are per tier and the tiers are in check/coverage.txt. The reason
# they are not one number over the whole repository is that the fastest route
# to a high number is executing code without asserting anything, and a single
# percentage rewards exactly that. A tier says how far out you have to reach to
# exercise a file at all, and a file that cannot be reached from CI is marked
# not applicable rather than counted as zero.

set -u

repo_root=$(git rev-parse --show-toplevel) || exit 1
cd "$repo_root" || exit 1

MANIFEST="check/coverage.txt"
PURE=95
INTEGRATION=70

if [ ! -f "$MANIFEST" ]; then
    echo "FAIL: no tier manifest at $MANIFEST" >&2
    exit 1
fi

echo "==> measuring"

report=$(mktemp)
trap 'rm -f "$report"' EXIT

# Vendored bindings are somebody else's source and are not Ward's to cover.
if ! cargo llvm-cov --summary-only --ignore-filename-regex 'crates[/\\]' >"$report" 2>&1; then
    echo "FAIL: the tests did not pass, so coverage means nothing" >&2
    cat "$report" >&2
    exit 1
fi

status=0

# --- every file is accounted for ---------------------------------------------
#
# Both directions. A new module with no line here would otherwise arrive with
# no floor and nothing would say so, and a line naming a file that no longer
# exists is a floor nobody is holding.

listed=$(sed -e 's/#.*//' -e '/^[[:space:]]*$/d' "$MANIFEST" | awk '{print $1}' | sort)
present=$(find src -maxdepth 1 -name '*.rs' -exec basename {} \; | sort)

missing=$(comm -13 "$(echo "$listed" >"$report.listed"; echo "$report.listed")" \
                   "$(echo "$present" >"$report.present"; echo "$report.present")")
extra=$(comm -23 "$report.listed" "$report.present")
rm -f "$report.listed" "$report.present"

if [ -n "$missing" ]; then
    echo "FAIL: these files are under src/ and have no tier:" >&2
    printf '%s\n' "$missing" | sed 's/^/    /' >&2
    echo "      add them to $MANIFEST" >&2
    status=1
fi

if [ -n "$extra" ]; then
    echo "FAIL: these are named in $MANIFEST and do not exist:" >&2
    printf '%s\n' "$extra" | sed 's/^/    /' >&2
    status=1
fi

# --- the floors --------------------------------------------------------------

echo "==> floors"

# The summary's line-coverage percentage is the tenth column. Anything that is
# not a source row is skipped, which drops the header, the rule and the total.
while read -r file tier rest; do
    case "$file" in ''|'#'*) continue ;; esac

    covered=$(awk -v f="$file" '$1 == f { gsub(/%/, "", $10); print $10 }' "$report")

    if [ -z "$covered" ]; then
        # A file with no executable lines at all never appears in the report.
        # That is not a failure; it is a file made entirely of declarations.
        printf '    %-18s %s\n' "$file" "not measured (no executable lines)"
        continue
    fi

    case "$tier" in
        pure)        floor=$PURE ;;
        integration) floor=$INTEGRATION ;;
        exempt)
            if [ -z "$rest" ]; then
                echo "    FAIL: $file is exempt and gives no reason" >&2
                status=1
            fi
            printf '    %-18s %6s%%  exempt\n' "$file" "$covered"
            continue
            ;;
        *)
            echo "    FAIL: $file has an unknown tier '$tier'" >&2
            echo "          expected pure, integration or exempt" >&2
            status=1
            continue
            ;;
    esac

    # Integer comparison on the whole-number part, so a floor of 95 is met by
    # 95.00 and not by 94.99. Shells cannot compare decimals and rounding up
    # would quietly move the floor.
    whole=${covered%%.*}

    if [ "$whole" -lt "$floor" ]; then
        printf '    %-18s %6s%%  BELOW %s%% (%s)\n' "$file" "$covered" "$floor" "$tier" >&2
        status=1
    else
        printf '    %-18s %6s%%  %s\n' "$file" "$covered" "$tier"
    fi
done <<EOF
$(sed -e 's/#.*//' -e '/^[[:space:]]*$/d' "$MANIFEST")
EOF

[ "$status" -eq 0 ] && echo "    ok"

exit "$status"
