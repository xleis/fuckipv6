package cc.xlei.fuckipv6.android.ui

import cc.xlei.fuckipv6.android.bridge.BridgeState
import cc.xlei.fuckipv6.android.bridge.NodeApiConfig

/**
 * What the app shell should show before the operator has a working node
 * connection.
 *
 * First launch has no configuration at all, which is not an outage: the shell
 * must invite the operator to enter a node address instead of rendering the
 * "node unavailable" surface, which would look like a broken install.
 */
enum class OnboardingStep {
    /** No address saved yet: the setup form is the whole screen. */
    CONFIGURE,

    /** An address is saved and the first snapshot has not arrived yet. */
    CONNECTING,

    /** The node answered: the normal shell takes over. */
    READY,

    /** A saved address is unreachable or rejected: keep the form, explain why. */
    FAILED,
}

data class OnboardingState(
    val step: OnboardingStep,
    /** Shared copy key for the explanation line, when there is one. */
    val messageKey: String? = null,
) {
    val showsForm: Boolean
        get() = step != OnboardingStep.READY

    val isBlocking: Boolean
        get() = step != OnboardingStep.READY
}

object OnboardingProjection {
    fun decide(config: NodeApiConfig?, bridge: BridgeState): OnboardingState {
        if (config == null || config.baseUrl.isBlank()) {
            return OnboardingState(
                step = OnboardingStep.CONFIGURE,
                messageKey = "onboarding.configure.detail",
            )
        }
        return when (bridge) {
            is BridgeState.Ready -> OnboardingState(step = OnboardingStep.READY)
            // The bridge starts in `NotConfigured` whenever it holds an HTTP
            // source, i.e. an address is saved but no snapshot has arrived yet.
            BridgeState.NotConfigured -> OnboardingState(step = OnboardingStep.CONNECTING)
            is BridgeState.Failed -> OnboardingState(
                step = OnboardingStep.FAILED,
                messageKey = bridge.failure.uiStateKey,
            )
        }
    }
}
