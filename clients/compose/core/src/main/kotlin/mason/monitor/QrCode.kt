// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

/**
 * A small QR encoder (s433): byte mode, error correction L or M, versions 1 to 40, the mask with
 * the lowest penalty. No dependency: the pairing URL is short ASCII. [modules] is the square of
 * dark (true) and light (false) modules, without the quiet zone.
 */
class QrCode private constructor(val version: Int, val modules: List<List<Boolean>>) {
    val size: Int get() = modules.size

    enum class Ecc(val format: Int) { L(1), M(0) }

    companion object {
        // Index by version (0 unused). Standard tables from ISO/IEC 18004.
        private val ECC_PER_BLOCK = mapOf(
            Ecc.L to intArrayOf(-1, 7, 10, 15, 20, 26, 18, 20, 24, 30, 18, 20, 24, 26, 30, 22, 24, 28, 30, 28, 28, 28, 28, 30, 30, 26, 28, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30),
            Ecc.M to intArrayOf(-1, 10, 16, 26, 18, 24, 16, 18, 22, 22, 26, 30, 22, 22, 24, 24, 28, 28, 26, 26, 26, 26, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28),
        )
        private val BLOCKS = mapOf(
            Ecc.L to intArrayOf(-1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 4, 4, 4, 4, 4, 6, 6, 6, 6, 7, 8, 8, 9, 9, 10, 12, 12, 12, 13, 14, 15, 16, 17, 18, 19, 19, 20, 21, 22, 24, 25),
            Ecc.M to intArrayOf(-1, 1, 1, 1, 2, 2, 4, 4, 4, 5, 5, 5, 8, 9, 9, 10, 10, 11, 13, 14, 16, 17, 17, 18, 20, 21, 23, 25, 26, 28, 29, 31, 33, 35, 37, 38, 40, 43, 45, 47, 49),
        )

        /** Data bytes a version holds at a level, as the standard tabulates (1-L is 19, 1-M is 16). */
        fun dataCodewords(version: Int, ecc: Ecc): Int =
            rawCodewords(version) - ECC_PER_BLOCK.getValue(ecc)[version] * BLOCKS.getValue(ecc)[version]

        private fun rawCodewords(v: Int): Int {
            var bits = (16 * v + 128) * v + 64
            if (v >= 2) {
                val a = v / 7 + 2
                bits -= (25 * a - 10) * a - 55
                if (v >= 7) bits -= 36
            }
            return bits / 8
        }

        /** Encode [text] (UTF-8 bytes). Throws IllegalArgumentException when it does not fit version 40. */
        fun encode(text: String, ecc: Ecc = Ecc.M): QrCode {
            val data = text.toByteArray(Charsets.UTF_8)
            var version = 1
            while (true) {
                require(version <= 40) { "text too long for a QR code" }
                val ccBits = if (version < 10) 8 else 16
                if (4 + ccBits + data.size * 8 <= dataCodewords(version, ecc) * 8) break
                version++
            }
            val cap = dataCodewords(version, ecc)
            val bits = ArrayList<Boolean>()
            fun put(v: Int, n: Int) { for (i in n - 1 downTo 0) bits.add((v shr i) and 1 == 1) }
            put(4, 4)
            put(data.size, if (version < 10) 8 else 16)
            data.forEach { put(it.toInt() and 0xFF, 8) }
            put(0, minOf(4, cap * 8 - bits.size))
            while (bits.size % 8 != 0) bits.add(false)
            var pad = 0xEC
            while (bits.size < cap * 8) { put(pad, 8); pad = pad xor (0xEC xor 0x11) }
            val bytes = IntArray(cap) { i -> (0 until 8).fold(0) { a, j -> (a shl 1) or (if (bits[i * 8 + j]) 1 else 0) } }
            return build(version, ecc, interleave(version, ecc, bytes))
        }

        private fun interleave(version: Int, ecc: Ecc, data: IntArray): IntArray {
            val nBlocks = BLOCKS.getValue(ecc)[version]
            val eccLen = ECC_PER_BLOCK.getValue(ecc)[version]
            val raw = rawCodewords(version)
            val shortBlocks = nBlocks - raw % nBlocks
            val shortLen = raw / nBlocks
            val div = reedSolomonDivisor(eccLen)
            val blocks = ArrayList<IntArray>()
            var k = 0
            for (i in 0 until nBlocks) {
                val len = shortLen - eccLen + (if (i < shortBlocks) 0 else 1)
                val d = data.copyOfRange(k, k + len)
                k += len
                val e = reedSolomonRemainder(d, div)
                val block = IntArray(shortLen + 1)
                // Short blocks keep a hole at index len so every block lines up when interleaved.
                d.copyInto(block)
                e.copyInto(block, shortLen + 1 - eccLen)
                blocks.add(block)
            }
            val out = ArrayList<Int>()
            for (i in 0..shortLen) {
                for ((j, b) in blocks.withIndex()) {
                    if (i != shortLen - eccLen || j >= shortBlocks) out.add(b[i])
                }
            }
            return out.toIntArray()
        }

        private fun gfMul(x: Int, y: Int): Int {
            var z = 0
            for (i in 7 downTo 0) {
                z = (z shl 1) xor ((z ushr 7) * 0x11D)
                z = z xor (((y ushr i) and 1) * x)
            }
            return z
        }

        private fun reedSolomonDivisor(degree: Int): IntArray {
            val r = IntArray(degree)
            r[degree - 1] = 1
            var root = 1
            repeat(degree) {
                for (j in 0 until degree) {
                    r[j] = gfMul(r[j], root)
                    if (j + 1 < degree) r[j] = r[j] xor r[j + 1]
                }
                root = gfMul(root, 2)
            }
            return r
        }

        private fun reedSolomonRemainder(data: IntArray, div: IntArray): IntArray {
            val r = IntArray(div.size)
            for (b in data) {
                val f = b xor r[0]
                for (i in 0 until r.size - 1) r[i] = r[i + 1]
                r[r.size - 1] = 0
                for (i in r.indices) r[i] = r[i] xor gfMul(div[i], f)
            }
            return r
        }

        private fun build(version: Int, ecc: Ecc, codewords: IntArray): QrCode {
            val n = version * 4 + 17
            val dark = Array(n) { BooleanArray(n) }
            val fn = Array(n) { BooleanArray(n) }
            fun set(x: Int, y: Int, v: Boolean) { dark[y][x] = v; fn[y][x] = true }

            for (i in 0 until n) { set(6, i, i % 2 == 0); set(i, 6, i % 2 == 0) }
            for ((cx, cy) in listOf(3 to 3, n - 4 to 3, 3 to n - 4)) {
                for (dy in -4..4) for (dx in -4..4) {
                    val x = cx + dx; val y = cy + dy
                    if (x in 0 until n && y in 0 until n) {
                        val d = maxOf(Math.abs(dx), Math.abs(dy))
                        set(x, y, d != 2 && d != 4)
                    }
                }
            }
            val pos = alignmentPositions(version, n)
            for (i in pos.indices) for (j in pos.indices) {
                if ((i == 0 && j == 0) || (i == 0 && j == pos.size - 1) || (i == pos.size - 1 && j == 0)) continue
                for (dy in -2..2) for (dx in -2..2) set(pos[i] + dx, pos[j] + dy, maxOf(Math.abs(dx), Math.abs(dy)) != 1)
            }
            drawFormat(dark, fn, n, ecc, 0)
            if (version >= 7) {
                var rem = version
                repeat(12) { rem = (rem shl 1) xor ((rem ushr 11) * 0x1F25) }
                val bits = (version shl 12) or rem
                for (i in 0 until 18) {
                    val b = (bits ushr i) and 1 == 1
                    val a = n - 11 + i % 3; val c = i / 3
                    set(a, c, b); set(c, a, b)
                }
            }
            // Data, zigzag from the bottom right.
            var i = 0
            var right = n - 1
            while (right >= 1) {
                if (right == 6) right = 5
                for (vert in 0 until n) for (j in 0..1) {
                    val x = right - j
                    val up = ((right + 1) and 2) == 0
                    val y = if (up) n - 1 - vert else vert
                    if (!fn[y][x] && i < codewords.size * 8) {
                        dark[y][x] = (codewords[i ushr 3] ushr (7 - (i and 7))) and 1 == 1
                        i++
                    }
                }
                right -= 2
            }
            var best = -1
            var bestScore = Int.MAX_VALUE
            for (m in 0 until 8) {
                applyMask(dark, fn, n, m)
                drawFormat(dark, fn, n, ecc, m)
                val s = penalty(dark, n)
                if (s < bestScore) { bestScore = s; best = m }
                applyMask(dark, fn, n, m)
            }
            applyMask(dark, fn, n, best)
            drawFormat(dark, fn, n, ecc, best)
            return QrCode(version, dark.map { r -> r.toList() })
        }

        private fun alignmentPositions(version: Int, n: Int): IntArray {
            if (version == 1) return IntArray(0)
            val num = version / 7 + 2
            val step = if (version == 32) 26 else (version * 4 + num * 2 + 1) / (num * 2 - 2) * 2
            val r = IntArray(num)
            r[0] = 6
            for (i in num - 1 downTo 1) r[i] = n - 7 - (num - 1 - i) * step
            return r
        }

        private fun drawFormat(dark: Array<BooleanArray>, fn: Array<BooleanArray>, n: Int, ecc: Ecc, mask: Int) {
            val data = (ecc.format shl 3) or mask
            var rem = data
            repeat(10) { rem = (rem shl 1) xor ((rem ushr 9) * 0x537) }
            val bits = ((data shl 10) or rem) xor 0x5412
            fun bit(i: Int) = (bits ushr i) and 1 == 1
            fun set(x: Int, y: Int, v: Boolean) { dark[y][x] = v; fn[y][x] = true }
            for (i in 0..5) set(8, i, bit(i))
            set(8, 7, bit(6)); set(8, 8, bit(7)); set(7, 8, bit(8))
            for (i in 9..14) set(14 - i, 8, bit(i))
            for (i in 0..7) set(n - 1 - i, 8, bit(i))
            for (i in 8..14) set(8, n - 15 + i, bit(i))
            set(8, n - 8, true)
        }

        private fun applyMask(dark: Array<BooleanArray>, fn: Array<BooleanArray>, n: Int, mask: Int) {
            for (y in 0 until n) for (x in 0 until n) {
                val inv = when (mask) {
                    0 -> (x + y) % 2 == 0
                    1 -> y % 2 == 0
                    2 -> x % 3 == 0
                    3 -> (x + y) % 3 == 0
                    4 -> (x / 3 + y / 2) % 2 == 0
                    5 -> x * y % 2 + x * y % 3 == 0
                    6 -> (x * y % 2 + x * y % 3) % 2 == 0
                    else -> ((x + y) % 2 + x * y % 3) % 2 == 0
                }
                if (inv && !fn[y][x]) dark[y][x] = !dark[y][x]
            }
        }

        private fun penalty(d: Array<BooleanArray>, n: Int): Int {
            var score = 0
            for (pass in 0..1) {
                fun at(a: Int, b: Int) = if (pass == 0) d[a][b] else d[b][a]
                for (a in 0 until n) {
                    var run = 1
                    for (b in 1 until n) {
                        if (at(a, b) == at(a, b - 1)) { run++; if (run == 5) score += 3 else if (run > 5) score++ } else run = 1
                    }
                }
            }
            for (y in 0 until n - 1) for (x in 0 until n - 1) {
                val c = d[y][x]
                if (c == d[y][x + 1] && c == d[y + 1][x] && c == d[y + 1][x + 1]) score += 3
            }
            val pat = booleanArrayOf(true, false, true, true, true, false, true)
            for (pass in 0..1) {
                fun at(a: Int, b: Int) = if (pass == 0) d[a][b] else d[b][a]
                for (a in 0 until n) for (b in 0..n - 7) {
                    if ((0 until 7).all { at(a, b + it) == pat[it] }) {
                        val before = (1..4).all { b - it < 0 || !at(a, b - it) }
                        val after = (7..10).all { b + it >= n || !at(a, b + it) }
                        if (before) score += 40
                        if (after) score += 40
                    }
                }
            }
            val total = n * n
            val k = d.sumOf { r -> r.count { it } }
            score += ((Math.abs(k * 20 - total * 10) + total - 1) / total - 1) * 10
            return score
        }
    }
}
