# tools

Scripts that produce committed artefacts. Each one says which question it answers and how
to run it; none of them are needed to build or use Slice, only to regenerate or re-check
something that is already in the tree.

| script | question it answers |
|---|---|
| `gen-solver-vectors.py` | Does our sub-space solver agree with fontTools' on every case fontTools tests? |

## `gen-solver-vectors.py`

`crates/slice-core/src/solver.rs` is a hand port of
`fontTools.varLib.instancer.solver`. The evidence that the port is faithful is
`crates/slice-core/src/solver_vectors.rs`: fontTools' own parametrised test table,
lifted verbatim and compiled into a Rust `const` that the solver's
`matches_fonttools_solver_test_vectors` test walks.

```sh
tools/gen-solver-vectors.py            # regenerate from the pinned fontTools tag
tools/gen-solver-vectors.py --check    # fail if the committed file is out of date
tools/gen-solver-vectors.py --from /path/to/solver_test.py   # use a local copy
```

The fontTools release is pinned in `FONTTOOLS_TAG` at the top of the script. To move to a
newer fontTools: bump the tag, re-run without `--check`, and commit the regenerated
vectors together with whatever solver change they force. The generated file records the
tag it came from in its header, so a checkout always says which upstream release its
vectors describe.

As of fontTools 4.62.1 the table holds 32 cases, and all 32 pass.
