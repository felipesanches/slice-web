# Test data

## `fonts/`

Copied verbatim from the original PyQt5 [Slice](https://github.com/source-foundry/Slice)
repository (`tests/assets/fonts/`) so that the Rust engine can be checked against the
behaviour of the application it replaces.

| file | what it is |
|---|---|
| `Recursive-VF.subset.ttf` | Recursive variable font, subset to a small charset. 5 axes: `MONO`, `CASL`, `wght`, `slnt`, `CRSV`. The primary instancing fixture. |
| `Recursive-VF.subset.woff` | Same font, WOFF-wrapped. Exercises the WOFF input path. |
| `Recursive-VF.subset.woff2` | Same font, WOFF2-wrapped. Exercises the WOFF2 input path (brotli). |
| `Recursive-Sliced.subset.ttf` | Output the original Slice produced from the fixture above. Used as a reference when comparing our instancing results. |

Recursive is by Stephen Nixon / ArrowType, licensed under the SIL Open Font License 1.1.

The axis extents of `Recursive-VF.subset.ttf`, as reported by its `fvar` table, are the
values the engine tests assert against:

```
MONO  0.0 : 1.0    [0.0]
CASL  0.0 : 1.0    [0.0]
wght  300.0 : 1000.0 [300.0]
slnt  -15.0 : 0.0  [0.0]
CRSV  0.0 : 1.0    [0.5]
```
