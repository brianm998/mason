# SPDX-License-Identifier: GPL-3.0-or-later
#
# The Compose monitor, always built as Compose's release build: the window and its
# libraries put through ProGuard (see desktop/build.gradle.kts).
#
#   make build   build the release jars into desktop/build/compose/tmp/main-release/proguard
#   make run     build them and start the window from them
#   make clean   remove every module's build products
#
# Pass arguments to the window with ARGS, e.g. `make run ARGS="--data-dir D"`.
#
# ProGuard reads the JDK's jmods, so the release build needs a full JDK 21 or later; the
# JetBrains runtime Android Studio ships has none. JDK= picks one, otherwise the first of:
# $JAVA_HOME, `java_home -v 21+`, Homebrew's openjdk.

GRADLE ?= gradle
ARGS   ?=
VERSION ?= $(shell sed -n 's/^version *= *"\([^"]*\)".*/\1/p' ../../Cargo.toml | head -1)

# $(call full_jdk,DIR): DIR if it is a JDK 21 or later with jmods, else nothing.
full_jdk = $(if $(and $(1),$(wildcard $(1)/jmods/java.base.jmod)),$(shell \
  v=$$(sed -n 's/^JAVA_VERSION="\([0-9]*\).*/\1/p' '$(1)/release' 2>/dev/null); \
  [ "$${v:-0}" -ge 21 ] && echo '$(1)'))
JDK ?= $(or $(call full_jdk,$(JAVA_HOME)), \
            $(call full_jdk,$(shell /usr/libexec/java_home -v 21+ 2>/dev/null)), \
            $(call full_jdk,$(shell brew --prefix openjdk 2>/dev/null)/libexec/openjdk.jdk/Contents/Home))
GRADLE_FLAGS := $(if $(strip $(JDK)),-Pmason.jdk=$(strip $(JDK))) -Pmason.version=$(VERSION)

ifneq ($(strip $(ARGS)),)
RUN_ARGS := --args='$(ARGS)'
endif

.PHONY: build run clean package-deb package-msi

build:
	$(GRADLE) $(GRADLE_FLAGS) :desktop:proguardReleaseJars

run:
	$(GRADLE) $(GRADLE_FLAGS) :desktop:runRelease $(RUN_ARGS)

# Installers need their own OS: .deb on Linux, .msi on Windows (WiX). Output is renamed into dist/.
package-deb:
	$(GRADLE) $(GRADLE_FLAGS) :desktop:packageReleaseDeb
	packaging/name-installers.sh $(VERSION)

package-msi:
	$(GRADLE) $(GRADLE_FLAGS) :desktop:packageReleaseMsi
	packaging/name-installers.sh $(VERSION)

clean:
	$(GRADLE) clean
