---
name: Bug report
about: Create a report to help us improve
title: ''
labels: ''
assignees: ''
---

## System Details (please complete the following information)

- OS Version: [e.g. mac 14.5 Sonoma, Linux Ubuntu 24.10]
- Processor: [e.g. M2, Intel Core i7-10750H]
- Architecture: [e.g. arm64, x86_64]

Example instructions to collect system details: _(Adjust if needed)_

Mac:

```bash
echo "OS Version: $(sw_vers -productName) $(sw_vers -productVersion)"
echo "Processor: $(sysctl -n machdep.cpu.brand_string)"
echo "Architecture: $(uname -m)"
```

Linux:

```bash
echo "OS Version: $(lsb_release -d | cut -f2)"
echo "CPU Architecture: $(uname -m)"
echo "Processor Model: $(grep -m 1 'model name' /proc/cpuinfo | cut -d: -f2-)"
```

- Browser [e.g. chrome , safari] (Optional)

## How are you running the node?

- [ ] merod binary _(obtained with homebrew or installation script)_
- [ ] building source code _(cloned github repository)_

> **Note:** If you are building from the source make sure you are using latest
> master

## Which tools versions you have?

- merod: _(get version by running "merod --version" in terminal)_
- meroctl: _(get version by running "meroctl --version" in terminal)_

## Describe the issue

A clear and concise description of what the bug is.

## How can we reproduce the issue

Steps to reproduce the behavior:

1. Node is stared with '...'
2. I did '...'
3. Then I did '...'
4. See error

## Expected behavior

A clear and concise description of what you expected to happen.

## Screenshots

If applicable, add screenshots to help explain your problem.

## Additional context

Add any other context about the problem here.
