# macbooks.json sources

Collected on 2026-11-26. Scope: every MacBook Air and MacBook Pro from 2018 to now (38 models).

## Apple
- Identify your MacBook Air model: https://support.apple.com/en-us/102869
- Identify your MacBook Pro model: https://support.apple.com/en-us/108052
  Used for: marketing names, model identifiers, colour lists, and the list of part numbers for each model.
- Tech Specs pages, one per model (the second URL in each model's `sources`), for example
  https://support.apple.com/en-us/111893 (MacBook Pro 13-inch M1). Used for: finishes, chips, display size
  and native resolution, ports, keyboard (Touch Bar / function row), and dimensions.
- Apple Store (US) buy pages and the product pages they link to:
  https://www.apple.com/shop/buy-mac/macbook-air, https://www.apple.com/shop/buy-mac/macbook-pro,
  https://www.apple.com/shop/buy-mac/macbook-pro-proled-xdr. Used for: part number to colour mapping for
  models on sale now (the product data gives each part number with its colour name).
- Apple Store colour swatch images (`store.storeimages.cdn-apple.com/.../<name>_SW_COLOR`). Used for: the
  approximate `body_hex` values. Each colour's `notes` field names the swatch it came from.

## everymac.com
everymac.com blocks automated requests, so its pages were read through the Internet Archive
(https://web.archive.org/web/2026/https://everymac.com/...), snapshots from Sep to Nov 2026:
- https://everymac.com/systems/apple/macbook_pro/index-macbookpro.html
- https://everymac.com/systems/apple/macbook-air/index-macbook-air.html
- The 86 per-configuration spec pages linked from those indexes for 2018 and later. Used for: part number to
  colour mapping (the order-number footnotes), and which model identifier goes with which chip or GPU.

## Cross-checks
- Each entry in `part_numbers` is confirmed by at least one of the two sources (the Apple Store, or an
  everymac footnote that names the colour for that exact number). `part_number_sources` records which.
  Where both sources cover the same number, they agree.
- Prefixes that Apple lists but no source gives a colour for are kept in `part_numbers_color_unknown`.
- Typos and oddities on Apple's identify pages are recorded in `apple_list_notes`, for example MYD83 (should
  be MYD82), MDH84 listed twice, and MGDQxx/A missing a digit.
