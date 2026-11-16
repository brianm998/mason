// SPDX-License-Identifier: GPL-3.0-or-later
package mason.android.ui

import android.Manifest
import android.content.pm.PackageManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import mason.android.PairingFlow
import mason.android.PairingScreenState
import mason.android.PairingScreenState.Path
import mason.android.ServiceSource

/** Shown when no daemon is paired: scan the QR code, or pick a nearby daemon and type its code. */
@Composable
fun PairingScreen(flow: PairingFlow, nearby: ServiceSource) {
    val s = flow.state
    // The Nearby list refreshes while the screen is up (NSD fills it in as daemons answer).
    var services by remember { mutableStateOf(nearby.services()) }
    LaunchedEffect(Unit) {
        while (true) {
            services = nearby.services().sortedBy { it.record.instance }
            kotlinx.coroutines.delay(1000)
        }
    }
    Column(Modifier.fillMaxSize().systemBarsPadding().padding(16.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(16.dp)) {
        Text("Pair with your MASON daemon", style = MaterialTheme.typography.titleLarge)
        SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
            SegmentedButton(selected = s.path == Path.Scan, onClick = { flow.update { it.withPath(Path.Scan) } }, shape = SegmentedButtonDefaults.itemShape(0, 2)) { Text("Scan") }
            SegmentedButton(selected = s.path == Path.Nearby, onClick = { flow.update { it.withPath(Path.Nearby) } }, shape = SegmentedButtonDefaults.itemShape(1, 2)) { Text("Nearby") }
        }
        when (s.path) {
            Path.Scan -> ScanPath(s) { text -> flow.update { it.scanned(text) } }
            Path.Nearby -> NearbyPath(s, services, flow)
        }
        OutlinedTextField(
            value = s.code, onValueChange = { v -> flow.update { it.withCode(v) } },
            label = { Text("8-digit code") }, singleLine = true,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.NumberPassword),
            modifier = Modifier.fillMaxWidth(),
        )
        s.error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
        Button(onClick = { flow.submit() }, enabled = s.canSubmit, modifier = Modifier.fillMaxWidth()) {
            Text(if (s.busy) "Pairing…" else "Pair")
        }
    }
}

@Composable
private fun ScanPath(s: PairingScreenState, onText: (String) -> Unit) {
    val a = s.address
    if (a != null) {
        Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Text(a.host, style = MaterialTheme.typography.titleMedium)
            Text("Fingerprint ${PairingScreenState.shortFingerprint(a.fingerprint)}…", style = MaterialTheme.typography.bodySmall, fontFamily = FontFamily.Monospace)
            Text("From a QR code or link: the full fingerprint is pinned.", style = MaterialTheme.typography.bodySmall)
        }
        return
    }
    val ctx = LocalContext.current
    var allowed by remember { mutableStateOf(ContextCompat.checkSelfPermission(ctx, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) }
    var asked by remember { mutableStateOf(false) }
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { allowed = it; asked = true }
    if (allowed) {
        QrScanner(onText, Modifier.fillMaxWidth().height(320.dp))
    } else {
        Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(
                if (asked) "The camera is not allowed. Allow it in Settings, open a mason://pair link, or use Nearby."
                else "Scanning the QR code from `mason pair` needs the camera.",
            )
            Button(onClick = { ask.launch(Manifest.permission.CAMERA) }) { Text("Allow the camera") }
        }
    }
}

@Composable
private fun NearbyPath(s: PairingScreenState, services: List<mason.android.ResolvedService>, flow: PairingFlow) {
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        if (services.isEmpty()) Text("Looking for daemons on this network…")
        for (r in services) {
            val picked = s.chosen?.record?.instance == r.record.instance
            OutlinedButton(onClick = { flow.update { it.choose(r) } }, modifier = Modifier.fillMaxWidth()) {
                Column(Modifier.weight(1f)) {
                    Text(r.record.instance, style = MaterialTheme.typography.titleSmall)
                    Text("Fingerprint ${PairingScreenState.shortFingerprint(r.record.fingerprint)}", style = MaterialTheme.typography.bodySmall, fontFamily = FontFamily.Monospace)
                }
                if (picked) Text("✓")
            }
        }
        Text(PairingScreenState.NEARBY_WARNING, style = MaterialTheme.typography.bodySmall)
    }
}
