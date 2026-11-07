// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import kotlin.math.max
import kotlin.math.min

/**
 * Horizontal placement of the pipeline graph (mirror of the Swift GraphLayout): canvas width and
 * the view-only x offset that keeps back-edge curves and labels unclipped and centers the graph.
 * Layout coordinates and the Move op never include this offset.
 */
object GraphLayout {
    const val NODE_W = 170.0
    const val LABEL_REACH = 100.0
    const val BULGE = 70.0
    const val PAD = 60.0
    const val MIN_WIDTH = 700.0

    data class BackEdge(val fromX: Double, val toX: Double, val labeled: Boolean)
    data class Horizontal(val width: Double, val dx: Double, val left: Double, val right: Double)

    fun horizontal(nodeXs: List<Double>, backEdges: List<BackEdge>, viewport: Double): Horizontal {
        var left = nodeXs.minOrNull() ?: 0.0
        var right = (nodeXs.maxOrNull() ?: 0.0) + NODE_W
        for (b in backEdges) {
            left = min(left, min(b.fromX - BULGE, b.toX - BULGE))
            if (b.labeled) left = min(left, b.fromX - LABEL_REACH)
        }
        right = max(right, left)
        val width = max(max(MIN_WIDTH, viewport), right - left + 2 * PAD)
        val dx = max(PAD - left, (width - (right + left)) / 2)
        return Horizontal(width, dx, left, right)
    }
}
