// SPDX-License-Identifier: GPL-3.0-or-later

import MonitorCore
import SwiftUI

private struct ScrollMetrics: Equatable {
    var offset: CGFloat = 0
    var height: CGFloat = 0
}

private struct ScrollMetricsKey: PreferenceKey {
    static let defaultValue = ScrollMetrics()
    static func reduce(value: inout ScrollMetrics, nextValue: () -> ScrollMetrics) { value = nextValue() }
}

/// A vertically scrolling page whose position survives back and forward. It reports its offset to
/// the model, and on arrival by back or forward puts the page where the history saved it: once,
/// when `ready` (the content has arrived), only if the owner has not scrolled meanwhile, clamped
/// to what is there. Later updates never move it again. The main view gives it a fresh identity
/// on every visit.
struct RestoringScroll<Content: View>: View {
    @ObservedObject var model: MonitorModel
    var ready: Bool = true
    @ViewBuilder var content: () -> Content

    @State private var target: Double
    @State private var settled: Bool
    @State private var metrics = ScrollMetrics()

    private static var space: String { "mason.scroll" }
    private static var marker: String { "mason.scroll.restore" }

    init(model: MonitorModel, ready: Bool = true, @ViewBuilder content: @escaping () -> Content) {
        self.model = model
        self.ready = ready
        self.content = content
        let t = model.history.restore
        _target = State(initialValue: t)
        _settled = State(initialValue: t <= 0)
    }

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                content()
                    .background(GeometryReader { g in
                        Color.clear.preference(
                            key: ScrollMetricsKey.self,
                            value: ScrollMetrics(offset: -g.frame(in: .named(Self.space)).minY, height: g.size.height))
                    })
                    .overlay(alignment: .topLeading) {
                        if !settled {
                            // Where the page should open: scrolling to this marker, which sits
                            // `target` down the content, puts the top of the view there.
                            Color.clear.frame(width: 1, height: 1)
                                .padding(.top, max(0, min(CGFloat(target), metrics.height - 1)))
                                .id(Self.marker)
                        }
                    }
            }
            .coordinateSpace(name: Self.space)
            .onPreferenceChange(ScrollMetricsKey.self) { m in
                metrics = m
                if settled { model.reportScroll(Double(m.offset)) }
            }
            .task(id: ready) {
                guard ready, !settled else { return }
                // Let the content lay out, then put the page back unless the owner has scrolled.
                try? await Task.sleep(nanoseconds: 60_000_000)
                if Task.isCancelled { return }
                if metrics.offset < 1 { proxy.scrollTo(Self.marker, anchor: .top) }
                settled = true
            }
        }
    }
}
