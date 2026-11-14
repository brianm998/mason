# SPDX-License-Identifier: GPL-3.0-or-later
#
# The SwiftUI monitor, always built with the release configuration.
#
#   make build   compile every product (-c release)
#   make run     build and start the macOS monitor window
#   make clean   remove the build products
#
# Pass arguments to the monitor with ARGS, e.g. `make run ARGS="--help"`.

SWIFT   ?= swift
CONFIG  := release
PRODUCT ?= MasonMonitor
ARGS    ?=

.PHONY: build run clean

# The commit that last changed the monitor goes into BuildStamp.swift for the build, so the
# binary knows it is out of date; the checked-in file is put back afterwards. A tree with
# uncommitted monitor changes is not stamped and stays "dev".
STAMP := Sources/MonitorCore/BuildStamp.swift
SHA   := $(shell git diff --quiet HEAD -- . ../../scripts/bundle-monitor.sh 2>/dev/null && git rev-list -1 HEAD -- . ../../scripts/bundle-monitor.sh 2>/dev/null)

build:
	@if [ -n "$(SHA)" ]; then \
	  printf '// SPDX-License-Identifier: GPL-3.0-or-later\n\npublic enum BuildStamp {\n    public static let commit = "%s"\n}\n' "$(SHA)" > $(STAMP); \
	fi; \
	$(SWIFT) build -c $(CONFIG); status=$$?; \
	git checkout -- $(STAMP) 2>/dev/null || true; \
	exit $$status

run: build
	$(SWIFT) run -c $(CONFIG) --skip-build $(PRODUCT) $(ARGS)

clean:
	$(SWIFT) package clean
