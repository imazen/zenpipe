# Adapter audit retained logs

- `/Users/lilith/work/codec-artifacts/zenpipe-arm-audit/zenpipe-arm-integration.log` SHA256 `af53978354dcb33698bc94d2426ad5bd170ef27096be55083b454ec612d1458f`
- `/Users/lilith/work/codec-artifacts/zenpipe-arm-audit/zenpipe-arm-integration-full.log` SHA256 `bd04e1cb3bb8acbb631fff0f3181e08b1dde0088b69e3fbacd449dd157462b59`

Source main `12b468e3`, existing Cargo.lock. `just arm-codec-integration-audit` adds the listed optional codec adapters. Full stdout/stderr retained, including the unchanged failing assertion.

- `/Users/lilith/work/codec-artifacts/zenpipe-arm-audit/zenpipe-preexisting-ci.log` SHA256 `a46d3e2c0ccfc4b83d4e7bd91198808d9548649324d8a47cae5ffb3b1a54e039` — complete pre-audit CI failure log.

- `/Users/lilith/work/codec-artifacts/zenpipe-arm-audit/zenpipe-pdf-final-integration.log` SHA256 `1f549585ef1ec386935fdad3e33921993f94727c69cb504c13e26ad51f94865e` — corrected PDF integration, 335 passed.
