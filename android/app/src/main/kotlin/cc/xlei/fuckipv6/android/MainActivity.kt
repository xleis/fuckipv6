package cc.xlei.fuckipv6.android

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import cc.xlei.fuckipv6.android.contract.SharedContractLoader
import cc.xlei.fuckipv6.android.ui.ContractFailureScreen
import cc.xlei.fuckipv6.android.ui.FuckIpv6App
import cc.xlei.fuckipv6.android.ui.FuckIpv6Theme

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        val contracts = runCatching { SharedContractLoader.load(assets) }
        setContent {
            FuckIpv6Theme {
                contracts.fold(
                    onSuccess = { FuckIpv6App(it) },
                    onFailure = { ContractFailureScreen(it.message) },
                )
            }
        }
    }
}
