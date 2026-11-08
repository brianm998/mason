// SPDX-License-Identifier: GPL-3.0-or-later

package mason.sample

import android.app.Activity
import android.os.Bundle
import android.widget.TextView

/** What the screen says. Kept out of the activity so a test can check it without a screen. */
fun greeting(name: String): String = if (name.isEmpty()) "Hello, stranger" else "Hello, $name"

class MainActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(TextView(this).apply { text = greeting("") })
    }
}
