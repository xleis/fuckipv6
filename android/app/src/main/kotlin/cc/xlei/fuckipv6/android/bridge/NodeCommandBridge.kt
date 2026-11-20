package cc.xlei.fuckipv6.android.bridge

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/** What the UI may claim about the last submitted command. */
sealed interface CommandState {
    data object Idle : CommandState
    data class InFlight(val operationId: String?, val stage: String) : CommandState
    data class Succeeded(val stage: String) : CommandState
    data class Rejected(
        val problemCode: String,
        val messageKey: String,
        val retryable: Boolean,
    ) : CommandState
    data class Failed(val failure: BridgeFailure) : CommandState

    val isSettled: Boolean
        get() = this is Succeeded || this is Rejected || this is Failed
}

/**
 * Submits one node command at a time and follows its operation to a terminal
 * state.
 *
 * Mutations are registered as operations before they run, so an accepted
 * submission is only `InFlight`; the bridge polls the authoritative operation
 * until it reports `succeeded`, a terminal failure, or the poll budget is spent.
 * Running out of polls is reported as `TimedOut`-style rejection instead of
 * pretending the command eventually succeeded.
 */
class NodeCommandBridge(
    private val transport: CommandTransport,
    private val pollIntervalMillis: Long = 500,
    private val maxPolls: Int = 60,
) {
    private val state = MutableStateFlow<CommandState>(CommandState.Idle)
    private val lastOutcome = MutableStateFlow<CommandOutcome?>(null)

    fun state(): StateFlow<CommandState> = state.asStateFlow()

    /**
     * Last raw outcome, including the payload the node returned. Read-only
     * consumers (for example the recovery list) need the data, not just the
     * stage; nobody may treat a non-terminal outcome as a result.
     */
    fun lastOutcome(): StateFlow<CommandOutcome?> = lastOutcome.asStateFlow()

    /** Submits [body] and follows the resulting operation. Returns the poll job. */
    fun submit(scope: CoroutineScope, body: String): Job = scope.launch {
        var outcome = transport.submit(body)
        var polls = 0
        while (true) {
            state.value = toState(outcome)
            lastOutcome.value = outcome
            val inFlight = outcome as? CommandOutcome.InProgress ?: return@launch
            val operationId = inFlight.operationId ?: return@launch
            if (polls >= maxPolls) {
                state.value = CommandState.Rejected(
                    problemCode = "node.error.operation_timeout",
                    messageKey = "state.error.detail",
                    retryable = true,
                )
                return@launch
            }
            delay(pollIntervalMillis)
            polls += 1
            outcome = transport.operation(operationId)
        }
    }

    companion object {
        fun toState(outcome: CommandOutcome): CommandState = when (outcome) {
            is CommandOutcome.Succeeded -> CommandState.Succeeded(outcome.stage)
            is CommandOutcome.InProgress -> CommandState.InFlight(outcome.operationId, outcome.stage)
            is CommandOutcome.Rejected -> CommandState.Rejected(
                outcome.problemCode,
                outcome.messageKey,
                outcome.retryable,
            )
            is CommandOutcome.Failed -> CommandState.Failed(outcome.failure)
        }
    }
}
