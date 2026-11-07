// SPDX-License-Identifier: GPL-3.0-or-later

/// Horizontal placement of the pipeline graph: how wide the canvas is and how far to shift
/// the drawing right so back-edge curves and their labels are never clipped on the left and
/// the graph is centered when the viewport is wider than it. View-only: layout coordinates
/// (and the Move op) never include this offset.
public enum GraphLayout {
    public static let nodeW = 170.0
    public static let labelReach = 100.0
    public static let bulge = 70.0
    public static let pad = 60.0
    public static let minWidth = 700.0

    public struct BackEdge: Equatable {
        public var fromX: Double
        public var toX: Double
        public var labeled: Bool
        public init(fromX: Double, toX: Double, labeled: Bool) { self.fromX = fromX; self.toX = toX; self.labeled = labeled }
    }

    public struct Horizontal: Equatable {
        public var width: Double
        public var dx: Double
        public var left: Double
        public var right: Double
    }

    public static func horizontal(nodeXs: [Double], backEdges: [BackEdge], viewport: Double) -> Horizontal {
        var left = nodeXs.min() ?? 0
        var right = (nodeXs.max() ?? 0) + nodeW
        for b in backEdges {
            left = min(left, b.fromX - bulge, b.toX - bulge)
            if b.labeled { left = min(left, b.fromX - labelReach) }
        }
        right = max(right, left)
        let width = max(minWidth, viewport, right - left + 2 * pad)
        let dx = max(pad - left, (width - (right + left)) / 2)
        return Horizontal(width: width, dx: dx, left: left, right: right)
    }
}
