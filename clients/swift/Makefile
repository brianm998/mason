# SPDX-License-Identifier: GPL-3.0-or-later
#
# The SwiftUI monitor, always built with the release configuration.
#
#   make build   compile every product (-c release)
#   make run     build and start the macOS monitor window
#   make clean   remove the build products
#   make version the commit `make run` reports (dev with local changes)
#
# Pass arguments to the monitor with ARGS, e.g. `make run ARGS="--help"`.

SWIFT   ?= swift
CONFIG  := release
PRODUCT ?= MasonMonitor
ARGS    ?=

# The commit this monitor is, so it can tell when a newer one is on the update channel:
# the last commit that changed it, by the same rule as scripts/bundle-monitor.sh. A
# checkout with uncommitted changes to these paths stays a development build ("dev"),
# which never asks to update. Override with `make run BUILD_SHA=...`.
VERSION_PATHS := clients/swift scripts/bundle-monitor.sh
REPO          := $(shell git rev-parse --show-toplevel 2>/dev/null)
BUILD_SHA     ?= $(if $(REPO),$(shell cd "$(REPO)" && git diff --quiet HEAD -- $(VERSION_PATHS) && git rev-list -1 HEAD -- $(VERSION_PATHS)))

.PHONY: build run clean version

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
	git checkout -- $(STAMP); \
	exit $$status

run: build
	$(if $(BUILD_SHA),MASON_MONITOR_VERSION=$(BUILD_SHA) )$(SWIFT) run -c $(CONFIG) --skip-build $(PRODUCT) $(ARGS)

# What `make run` will report as this monitor's version.
version:
	@echo $(or $(BUILD_SHA),dev)

clean:
	$(SWIFT) package clean
