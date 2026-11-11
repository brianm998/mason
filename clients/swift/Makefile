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

build:
	$(SWIFT) build -c $(CONFIG)

run: build
	$(SWIFT) run -c $(CONFIG) --skip-build $(PRODUCT) $(ARGS)

clean:
	$(SWIFT) package clean
