export type OwnerSession = {
  authenticated: boolean;
  expected_origin?: string;
  credential_id?: string;
  channel?: string;
  expires_at_unix_ms?: number;
  last_authenticated_at_unix_ms?: number;
};

type PublicKeyOptions = PublicKeyCredentialCreationOptions | PublicKeyCredentialRequestOptions;

function base64urlToBuffer(value: string): ArrayBuffer {
  if (!value) throw new Error("WebAuthn returned an empty base64url value");
  const normalized = value.replace(/-/g, "+").replace(/_/g, "/");
  const padded = normalized + "=".repeat((4 - (normalized.length % 4)) % 4);
  const binary = atob(padded);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return bytes.buffer;
}

function bufferToBase64url(value: ArrayBuffer): string {
  const bytes = new Uint8Array(value);
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 0x8000));
  }
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/g, "");
}

function challengePayload(payload: unknown): { publicKey: Record<string, unknown> } {
  if (typeof payload !== "object" || payload === null || !("publicKey" in payload)) {
    throw new Error("WebAuthn challenge is missing publicKey options");
  }
  const publicKey = (payload as { publicKey: unknown }).publicKey;
  if (typeof publicKey !== "object" || publicKey === null) {
    throw new Error("WebAuthn challenge publicKey is invalid");
  }
  return { publicKey: publicKey as Record<string, unknown> };
}

export function decodeRequestOptions(payload: unknown): PublicKeyCredentialRequestOptions {
  const options = challengePayload(payload).publicKey;
  const challenge = options.challenge;
  if (typeof challenge !== "string") throw new Error("WebAuthn challenge is missing");
  const decoded: Record<string, unknown> = {
    ...options,
    challenge: base64urlToBuffer(challenge),
  };
  if (Array.isArray(options.allowCredentials)) {
    decoded.allowCredentials = options.allowCredentials.map((credential) => {
      const item = credential as Record<string, unknown>;
      return { ...item, id: base64urlToBuffer(String(item.id ?? "")) };
    });
  }
  return decoded as unknown as PublicKeyCredentialRequestOptions;
}

export function decodeCreationOptions(payload: unknown): PublicKeyCredentialCreationOptions {
  const options = challengePayload(payload).publicKey;
  const challenge = options.challenge;
  const user = options.user as Record<string, unknown> | undefined;
  if (typeof challenge !== "string" || !user || typeof user.id !== "string") {
    throw new Error("WebAuthn creation challenge is incomplete");
  }
  const decoded: Record<string, unknown> = {
    ...options,
    challenge: base64urlToBuffer(challenge),
    user: { ...user, id: base64urlToBuffer(user.id) },
  };
  if (Array.isArray(options.excludeCredentials)) {
    decoded.excludeCredentials = options.excludeCredentials.map((credential) => {
      const item = credential as Record<string, unknown>;
      return { ...item, id: base64urlToBuffer(String(item.id ?? "")) };
    });
  }
  return decoded as unknown as PublicKeyCredentialCreationOptions;
}

export function encodeAuthenticationCredential(credential: PublicKeyCredential): Record<string, unknown> {
  const response = credential.response as AuthenticatorAssertionResponse;
  const encoded: Record<string, unknown> = {
    id: credential.id,
    rawId: bufferToBase64url(credential.rawId),
    type: credential.type,
    response: {
      authenticatorData: bufferToBase64url(response.authenticatorData),
      clientDataJSON: bufferToBase64url(response.clientDataJSON),
      signature: bufferToBase64url(response.signature),
    },
    clientExtensionResults: credential.getClientExtensionResults(),
  };
  if (response.userHandle) {
    (encoded.response as Record<string, unknown>).userHandle = bufferToBase64url(response.userHandle);
  }
  return encoded;
}

export function encodeRegistrationCredential(credential: PublicKeyCredential): Record<string, unknown> {
  const response = credential.response as AuthenticatorAttestationResponse;
  const transports = response.getTransports?.();
  return {
    id: credential.id,
    rawId: bufferToBase64url(credential.rawId),
    type: credential.type,
    response: {
      attestationObject: bufferToBase64url(response.attestationObject),
      clientDataJSON: bufferToBase64url(response.clientDataJSON),
      ...(transports ? { transports } : {}),
    },
    clientExtensionResults: credential.getClientExtensionResults(),
  };
}

export function webAuthnIssue(expectedOrigin?: string): string | null {
  if (!window.isSecureContext) return "WebAuthn requires a secure context";
  if (!window.PublicKeyCredential || !navigator.credentials) return "This browser does not support Passkeys";
  if (expectedOrigin && window.origin !== expectedOrigin) {
    return `Passkey origin mismatch: expected ${expectedOrigin}, current ${window.origin}`;
  }
  return null;
}

export function isCredential(value: Credential | null): value is PublicKeyCredential {
  return value !== null && value.type === "public-key";
}

export type { PublicKeyOptions };
