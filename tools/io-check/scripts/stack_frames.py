"""Stack frames at a stack-overflow abort, with each frame's size.

Run inside gdb against a built test binary (no stack-size override):

    gdb -q -batch -x tools/io-check/scripts/stack_frames.py \
        --args <test-binary> <test-name> --exact --test-threads 1

Stops at the first SIGSEGV (the guard-page hit that Rust reports as "has
overflowed its stack"), then prints every frame of the faulting thread with
its size (stack-pointer distance to the next older frame), the total span, the
most repeated functions (recursion shows up here) and the largest frames.
`just io-stack-frames` builds the io-check tests and runs this per test.
"""
import gdb, collections
gdb.execute("set pagination off")
gdb.execute("set print frame-arguments none")
gdb.execute("handle SIGSEGV stop nopass")
gdb.execute("run")
th = gdb.selected_thread()
print("THREAD", th.num, th.name)
rows = []
f = gdb.newest_frame()
while f is not None:
    try:
        sp = int(f.read_register("rsp"))
    except Exception:
        sp = None
    name = f.name() or "??"
    sal = f.find_sal()
    loc = f"{sal.symtab.filename}:{sal.line}" if sal and sal.symtab else ""
    rows.append((name, sp, loc))
    f = f.older()
print("FRAMES", len(rows))
sizes = []
for i, (name, sp, loc) in enumerate(rows):
    nxt = rows[i + 1][1] if i + 1 < len(rows) else None
    size = (nxt - sp) if (sp is not None and nxt is not None) else 0
    sizes.append(size)
top = rows[0][1]; bottom = rows[-1][1]
print("SPAN_BYTES", (bottom - top) if (top and bottom) else None)
for i, (name, sp, loc) in enumerate(rows):
    print(f"#{i:<4} {sizes[i]:>9} {name[:150]}  {loc[-90:]}")
counts = collections.Counter(n for n, _, _ in rows)
print("MOST_REPEATED", counts.most_common(8))
big = sorted(((s, n, l) for s, (n, _, l) in zip(sizes, rows)), reverse=True)[:15]
print("LARGEST_FRAMES")
for s, n, l in big:
    print(f"  {s:>9} {n[:150]}  {l[-90:]}")
