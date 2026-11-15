// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay
import mason.monitor.DeviceList
import mason.monitor.LanStatus
import mason.monitor.MonitorModel
import mason.monitor.PairOffer
import mason.monitor.PairedDevice
import mason.monitor.QrCode

/**
 * Settings > Devices (s433): whether LAN access is on, the paired devices with Revoke (after a
 * confirmation), and "Pair a device…", which shows the `mason://pair` URL as a QR code with the
 * 8-digit code.
 */
@Composable
fun DevicesSection(model: MonitorModel) {
    val theme = LocalTheme.current
    var pairing by remember { mutableStateOf(false) }
    var revoking by remember { mutableStateOf<PairedDevice?>(null) }
    Section("Devices") {
        model.devicesNotice?.let { Text(it, style = theme.body(color = theme.muted), modifier = Modifier.testTag("devices:notice")) }
        val lan = model.lanStatus
        if (lan != null) {
            if (lan.enabled) {
                Text("LAN access is on" + (if (lan.hosts.isEmpty()) "." else ": " + lan.hosts.joinToString(", ")), style = theme.body(), modifier = Modifier.testTag("devices:lan"))
                TextButton(onClick = { pairing = true }, modifier = Modifier.testTag("devices:pair")) { Text("Pair a device…") }
            } else {
                Text(LanStatus.HOW_TO_TURN_ON, style = theme.body(color = theme.muted), modifier = Modifier.testTag("devices:lan"))
            }
        }
        model.devices?.let { list ->
            if (list.active.isEmpty() && list.revoked.isEmpty()) Text("No device is paired.", style = theme.body(color = theme.muted))
            list.active.forEach { d ->
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(theme.spacing)) {
                    Text("${d.name} (${d.role}), last seen ${DeviceList.lastSeenText(d.lastSeen, model.daemonNow())}", style = theme.body())
                    TextButton(onClick = { revoking = d }, modifier = Modifier.testTag("devices:revoke:${d.id}")) { Text("Revoke") }
                }
            }
            if (list.revoked.isNotEmpty()) {
                Text("Revoked", style = theme.small(color = theme.muted))
                list.revoked.forEach { Text("${it.name} (${it.role}), revoked", style = theme.body(color = theme.muted)) }
            }
        }
    }
    revoking?.let { d ->
        AlertDialog(
            onDismissRequest = { revoking = null },
            title = { Text("Revoke this device?", style = theme.title()) },
            text = { Text("${d.name} will be signed out and must pair again.", style = theme.body()) },
            confirmButton = { TextButton(onClick = { model.revokeDevice(d.id); revoking = null }, modifier = Modifier.testTag("devices:revoke-confirm")) { Text("Revoke") } },
            dismissButton = { TextButton(onClick = { revoking = null }) { Text("Cancel") } },
        )
    }
    if (pairing) PairDialog(model) { pairing = false; model.closePairing() }
}

/** The pairing dialog: a role, a QR code, the code in large type, the hosts and a countdown that offers a new code when it runs out. While it is open the device list is re-read every 3 s. */
@Composable
private fun PairDialog(model: MonitorModel, onClose: () -> Unit) {
    val theme = LocalTheme.current
    var viewer by remember { mutableStateOf(false) }
    var now by remember { mutableStateOf(System.currentTimeMillis()) }
    val role = if (viewer) "viewer" else "owner"
    LaunchedEffect(Unit) {
        if (model.pairOffer == null) model.issuePairCode(role)
        var tick = 0
        while (true) {
            delay(1000)
            now = System.currentTimeMillis()
            tick++
            if (tick % 3 == 0) model.loadDevices()
        }
    }
    AlertDialog(
        onDismissRequest = onClose,
        title = { Text("Pair a device", style = theme.title()) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(theme.spacing), horizontalAlignment = Alignment.CenterHorizontally) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    TextButton(onClick = { viewer = false; model.closePairing(); model.issuePairCode("owner") }, modifier = Modifier.testTag("devices:role:owner")) { Text(if (!viewer) "[Owner]" else "Owner") }
                    TextButton(onClick = { viewer = true; model.closePairing(); model.issuePairCode("viewer") }, modifier = Modifier.testTag("devices:role:viewer")) { Text(if (viewer) "[Viewer]" else "Viewer") }
                }
                val o = model.pairOffer
                if (o != null) {
                    QrImage(o.url, Modifier.size(240.dp).testTag("devices:qr"))
                    SelectionContainer { Text(o.code, style = theme.title().copy(fontSize = 40.sp, fontWeight = FontWeight.Bold, fontFamily = FontFamily.Monospace), modifier = Modifier.testTag("devices:code")) }
                    Text(o.hosts.joinToString(", "), style = theme.small(color = theme.muted))
                    when (val s = o.state(now)) {
                        is PairOffer.State.Valid -> Text("Expires in ${PairOffer.clock(s.secondsLeft)}", style = theme.body(), modifier = Modifier.testTag("devices:countdown"))
                        PairOffer.State.Expired -> {
                            Text("This code has expired.", style = theme.body(color = theme.muted))
                            TextButton(onClick = { model.issuePairCode(role) }, modifier = Modifier.testTag("devices:new-code")) { Text("New code") }
                        }
                    }
                } else {
                    TextButton(onClick = { model.issuePairCode(role) }, modifier = Modifier.testTag("devices:new-code")) { Text("Make a code") }
                }
                model.devicesNotice?.let { Text(it, style = theme.small(color = theme.muted)) }
            }
        },
        confirmButton = { TextButton(onClick = onClose) { Text("Done") } },
    )
}

/** [text] as a QR code on a white square with a 4-module quiet zone. */
@Composable
fun QrImage(text: String, modifier: Modifier = Modifier) {
    val qr = remember(text) { runCatching { QrCode.encode(text) }.getOrNull() } ?: return
    Canvas(modifier) {
        val total = qr.size + 8
        val cell = minOf(size.width, size.height) / total
        drawRect(Color.White, Offset.Zero, Size(cell * total, cell * total))
        for (y in 0 until qr.size) for (x in 0 until qr.size) {
            if (qr.modules[y][x]) drawRect(Color.Black, Offset((x + 4) * cell, (y + 4) * cell), Size(cell, cell))
        }
    }
}
