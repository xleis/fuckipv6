package cc.xlei.fuckipv6.android.bridge

import java.io.BufferedInputStream
import java.io.ByteArrayOutputStream
import java.io.InputStream
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.util.concurrent.CopyOnWriteArrayList

/**
 * Tiny blocking HTTP/1.1 server for unit tests.
 *
 * The Android unit-test classpath has no `com.sun.net.httpserver`, so this
 * helper speaks just enough HTTP to let the production clients make real
 * requests: one request per connection, fixed status/body, `Connection: close`.
 */
class TestHttpServer(
    private val handler: (Request) -> Response,
) : AutoCloseable {
    data class Request(
        val method: String,
        val path: String,
        val query: String,
        val headers: Map<String, String>,
        val body: String,
    )

    data class Response(
        val status: Int,
        val body: String,
        val contentType: String = "application/json",
        /**
         * Overrides the advertised `Content-Length` while the body stays short,
         * so tests can reproduce a stream that dies mid-transfer.
         */
        val declaredLength: Int? = null,
    )

    private val serverSocket = ServerSocket(0, 8, InetAddress.getLoopbackAddress())
    val port: Int = serverSocket.localPort
    val requests = CopyOnWriteArrayList<Request>()

    @Volatile
    private var running = true

    private val worker = Thread {
        while (running) {
            val socket = try {
                serverSocket.accept()
            } catch (_: Exception) {
                return@Thread
            }
            runCatching { serve(socket) }
        }
    }.apply {
        isDaemon = true
        name = "test-http-server"
        start()
    }

    private fun serve(socket: Socket) {
        socket.use { connection ->
            connection.soTimeout = 5_000
            val input = BufferedInputStream(connection.getInputStream())
            val requestLine = readLine(input) ?: return
            val parts = requestLine.split(' ')
            if (parts.size < 2) return
            val target = parts[1]
            val path = target.substringBefore('?')
            val query = target.substringAfter('?', "")
            val headers = mutableMapOf<String, String>()
            while (true) {
                val line = readLine(input) ?: break
                if (line.isEmpty()) break
                val separator = line.indexOf(':')
                if (separator > 0) {
                    headers[line.substring(0, separator).lowercase()] =
                        line.substring(separator + 1).trim()
                }
            }
            val length = headers["content-length"]?.toIntOrNull() ?: 0
            val body = readBody(input, length).toString(Charsets.UTF_8)
            val request = Request(parts[0], path, query, headers, body)
            requests += request
            val response = handler(request)
            val bytes = response.body.toByteArray(Charsets.UTF_8)
            val head = buildString {
                append("HTTP/1.1 ${response.status} ${statusText(response.status)}\r\n")
                append("Content-Type: ${response.contentType}\r\n")
                append("Content-Length: ${response.declaredLength ?: bytes.size}\r\n")
                append("Connection: close\r\n\r\n")
            }
            connection.getOutputStream().let { output ->
                output.write(head.toByteArray(Charsets.US_ASCII))
                output.write(bytes)
                output.flush()
            }
        }
    }

    private fun readLine(input: InputStream): String? {
        val buffer = ByteArrayOutputStream()
        while (true) {
            val byte = input.read()
            if (byte < 0) return if (buffer.size() == 0) null else buffer.toString("US-ASCII")
            if (byte == '\n'.code) {
                val text = buffer.toString("US-ASCII")
                return text.trimEnd('\r')
            }
            buffer.write(byte)
        }
    }

    private fun readBody(input: InputStream, length: Int): ByteArray {
        if (length <= 0) return ByteArray(0)
        val buffer = ByteArray(length)
        var read = 0
        while (read < length) {
            val chunk = input.read(buffer, read, length - read)
            if (chunk < 0) break
            read += chunk
        }
        return buffer.copyOf(read)
    }

    private fun statusText(status: Int): String = when (status) {
        200 -> "OK"
        201 -> "Created"
        401 -> "Unauthorized"
        403 -> "Forbidden"
        404 -> "Not Found"
        429 -> "Too Many Requests"
        500 -> "Internal Server Error"
        else -> "Status"
    }

    override fun close() {
        running = false
        runCatching { serverSocket.close() }
    }
}
