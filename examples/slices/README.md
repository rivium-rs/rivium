# Validation slices

Each slice is a small service written the way a service built on Rivium is: the template's
layout, with a service library that holds the business and the composition root, a program, and
a JNI library when the service also runs inside an Android app. A slice checks that Rivium's
contract and API express such a service without changes to Rivium. Besides each slice's own
tests, `ci/slices` checks that:

- every program depends on nothing but its service library and `rivium`, every JNI library on
  nothing but its service library and `rivium-jni`, and each has at most 10 lines of code
  (`scripts/check-slice-deps.sh`);
- the same business tests pass through the process host and, for `snmp-lite`, through the
  embedded host in a desktop JVM (`snmp-lite-bin/tests/hosts.rs`);
- `snmp-lite` answers net-snmp's `snmpget`, `snmpgetnext` and `snmpwalk`
  (`scripts/snmp-interop.sh`).

`just slices` runs these checks locally in a Linux container.

| Slice | What it is |
| --- | --- |
| [`snmp-lite`](snmp-lite) | An SNMPv2c agent that answers GET and GETNEXT for a few scalars (its own below the enterprise number 32473, which RFC 5612 reserves for documentation), and a job that polls a simulated device with a blocking read. It runs as a program (`snmp-lite-bin`) and inside an Android app (`snmp-lite-jni`), which passes the device's serial number |
| [`edge-lite`](edge-lite) | An edge collector: units that poll simulated devices under the collector's own supervision (a unit that panics or fails starts again, one that keeps failing is given up), a store whose last write ends before the stop deadline, an HTTP API with the status envelope, a log file of the units' own with its export, and a configuration that the API replaces and a restart loads. It runs as a program (`edge-lite-bin`) |
