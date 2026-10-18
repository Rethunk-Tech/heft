# TODO

[HUMANS.md](HUMANS.md) and [AGENTS.md](AGENTS.md) describe what heft does.

## Ryzen AI NPU — measured, not worth a column

Unprivileged (uid 1000) on this Strix Halo host, 2026-10-18. PCI `0000:c3:00.1` is class `0x118000`, vendor `0x1022`, device `0x17f0`, driver `amdxdna`; `vbnv` reads `NPU Strix Halo\n`.

`open("/sys/class/accel/accel0/dev", O_RDONLY)` returned `261:0\n`. `open(".../uevent")` returned `MAJOR=261\nMINOR=0\nDEVNAME=accel/accel0\nDEVTYPE=accel_minor\n`. That sysfs node has no utilization, busy, or load attribute. `open("/sys/kernel/debug/amdxdna")` and `open("/sys/kernel/debug/accel")` returned `EACCES` (13). No `/proc/<pid>/fdinfo` contained `amdxdna`, `accel`, or `npu` keys, so the `drm-client-id` + `drm-resident-*` fallback is not billing this device under VRAM/GTT. `open("/dev/accel/accel0")` returned `ENOENT` (2) in this process's `/dev`.

No column. `mem_key` / `drm-driver` unchanged. A root debugfs read is a different program.

## NVIDIA and nouveau fdinfo — measured, not worth new keys

Unprivileged (uid 1000) on this amdgpu APU host, 2026-10-18. `/sys/module/nvidia` and `/sys/module/nouveau` are absent. `open("/dev/nvidia0")`, `open("/dev/nvidiactl")`, and `open("/proc/driver/nvidia")` each returned `ENOENT` (2). A scan of `/proc/<pid>/fdinfo` found no `drm-driver: nvidia` or `drm-driver: nouveau` line, so the resident fallback is not holding a blank NVIDIA/nouveau client either.

No region or engine names to map. `mem_key` and `drm-driver` stay amdgpu/i915/xe. Guessed keys from a kernel tree stay unread.
