# Verification against real data (Trolleys)

Run against `Customer Projects\Trolleys` (read-only) into a scratch DB. Command: `mtd index <Trolleys> --family Trolleys`.

## Counts

| | |
|---|---|
| files seen | 138 (124 indexed, 14 ignored: 10 `Thumbs.db`, 4 `~$` SolidWorks lock files) |
| jobs | 3: J006635 (8 files, 3 designs), J007097 (112 files, 68 designs), J007262 (4 files, 2 designs) |
| designs | 73 (45 numbered-part designs + descriptive/attic designs) |
| facts / chunks | 780 / 203 |
| extractor errors | 0 |
| awaiting SolidWorks analysis | 87 (24 asm, 51 part, 12 drawings; expected, no geometry yet) |

Classification by kind: assembly 24, part 51, drawing 12, pdf 15, image 18, document 2, sheet 1, step 1. No file landed in an unexpected kind.

## Part-number parsing

All forms in the data parse: `_R0`, `_R00` (normalised to R0), `-R0` (`J007097-01C-00-R0.PDF`), no revision (`J007097-01C-00.SLDDRW`). Descriptive names (`SHELF`, `BOTTLE`, `WALL A`, `FLOOR PLAN`, `CASTORS`) get a `JOB:name` key, with `ATTIC/<subfolder>/` scoping so same-named attic files do not collide. Variants A-E with item 00 = top-level assembly are correct; the BOM-style titles (TOTE TROLLEY, TOTE SHELF, BOTTLE DRAWER, BAG DRAWER, TOTE LOADING TROLLEY) are recovered from drawings and `PART WEIGHTS.xlsx`.

J007097/3D/ATTIC contains `J007079-*` files (17 designs): a job-number typo; flagged with `identity.note` and kept as separate superseded designs.

## Series 00 vs 01 (open question)

Evidence: series `01` exists only as top-level (item `00`) assemblies for variants B, C, E (`01B-00`, `01C-00`, `01E-00`; 01C also has a drawing and PDF). No `01` parts exist, so 01 assemblies reuse the `00` parts. ATTIC has an "SS VERSION" / "SS V2" tree and `J007097 JPG/FOR EMAIL` has PLASTIC VERSION and STAINLESS VERSION images, and `TOTE BOX COMPARISON.docx` compares old grey vs new green plastic tote boxes. Most likely reading: `00` = first (plastic-tote / base) configuration, `01` = the alternate (stainless) configuration of the same product variant. This is inferred, not stated anywhere in the files; the parser still stores the series verbatim. Confirm with the user, or by inspecting the 01 assemblies' materials once SolidWorks analysis is available.

## Recall checks

- `mtd search "castor"` finds the CASTORS STEP, the recommended-castor SLDDRW and PDF, and the PDF page text (BICKLE LX-PO 100G-FI-ELS, part 879423, height 125, wheel dia 100).
- `mtd search "what is the load per drawer"` returns the `PART WEIGHTS.xlsx` rows; `mtd card J007097-00C-00` shows load per drawer 80 kg, net load 332 kg, trolley 172.5 kg with cell provenance (H4, I4, F4).
- `mtd facts "sheet.%weight%" --min 100` lists the five trolley weights (112 to 209.6 kg).
- `mtd search "stainless"` hits the image filenames; `mtd search "01C"` finds the 01C files.
- `mtd ask "What castor is recommended for J007097 and what is its load rating?" --model haiku` worked end to end (about $0.005, 5 tool calls): answered BICKLE LX-PO 100G-FI-ELS / 879423, 125 mm, 100 mm wheel, and honestly flagged the "250" load rating as unit-less.

## Bugs found and fixed

1. Descriptively named published PDFs (`J007097-TOTE BOX.pdf`, `J007097- RECOMMENDED CASTOR.pdf`) got no design key while their SLDDRW did, so a drawing and its PDF were not linked. Fix in `crates/mtd-index/src/scan.rs` (`design_key_for`): a PDF in the PDF zone shares the descriptive key of its drawing. PDFs in the JPG folder (catalogues, comparisons) remain job-level. Regression test added to `design_keys`.

## Remaining gaps

- Castor PDF load rating "250" is only in the text chunk; no `pdf` fact or unit is derived ("LOAD RATING (PER CASTOR) 250 kg" is split across lines in extraction).
- Variant roll-ups in the job card count the typo'd `J007079-00A/00B` designs under variants 00A/00B (00A shows 12 designs, 10 of them J007079). Consider excluding `job_mismatch` designs or labelling them.
- `get_job` for J007097 returns about 10 KB (lists all 68 designs); may be heavy for the MCP path. Consider collapsing attic designs.
- Titles for descriptive drawings keep the job prefix (`J007097- RECOMMENDED CASTOR`); cosmetic.
- 01-series designs have no titles ("untitled"); could inherit the 00 variant title.
- No geometry/materials/mass for 87 SolidWorks files until the SolidWorks analysis stage exists.
- Scanned/imagery content (JPG renders, images in docx) is not interpreted.
