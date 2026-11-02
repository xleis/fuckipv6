import { apiRequest, requestJson } from "./client";
import {
  decodeCreationOptions,
  decodeRequestOptions,
  encodeAuthenticationCredential,
  encodeRegistrationCredential,
  isCredential,
  type OwnerSession,
} from "../auth/webauthn";

export type LanguagePreference = "system" | "zh-CN" | "en";

export type LanguageSettings = {
  preference: LanguagePreference;
};

export async function loadOwnerSession(): Promise<OwnerSession> {
  const { response, payload } = await requestJson<OwnerSession>("/api/v1/auth/session");
  if (!response.ok) throw new Error(`Unable to read owner session (HTTP ${response.status})`);
  if (!payload || typeof payload.authenticated !== "boolean") throw new Error("Owner session response is invalid");
  return payload;
}

export async function loginWithPasskey(): Promise<OwnerSession> {
  const start = await apiRequest<unknown>("/api/v1/auth/passkey/authentication/start", { method: "POST" });
  const credential = await navigator.credentials.get({ publicKey: decodeRequestOptions(start) });
  if (!isCredential(credential)) throw new Error("Passkey authentication was cancelled");
  await apiRequest<unknown>("/api/v1/auth/passkey/authentication/finish", {
    method: "POST",
    body: JSON.stringify({ credential: encodeAuthenticationCredential(credential) }),
  });
  const session = await loadOwnerSession();
  if (!session.authenticated) throw new Error("The owner session was not established");
  return session;
}

export async function reauthenticateWithPasskey(): Promise<OwnerSession> {
  const start = await apiRequest<unknown>("/api/v1/auth/passkey/reauth/start", { method: "POST" });
  const credential = await navigator.credentials.get({ publicKey: decodeRequestOptions(start) });
  if (!isCredential(credential)) throw new Error("Passkey reauthentication was cancelled");
  await apiRequest<unknown>("/api/v1/auth/passkey/reauth/finish", {
    method: "POST",
    body: JSON.stringify({ credential: encodeAuthenticationCredential(credential) }),
  });
  const session = await loadOwnerSession();
  if (!session.authenticated) throw new Error("The owner session was not reestablished");
  return session;
}

export async function registerPasskey(label: string): Promise<unknown> {
  const start = await apiRequest<unknown>("/api/v1/auth/passkeys/registration/start", {
    method: "POST",
    body: JSON.stringify({ label }),
  });
  const credential = await navigator.credentials.create({ publicKey: decodeCreationOptions(start) });
  if (!isCredential(credential)) throw new Error("Passkey registration was cancelled");
  return apiRequest("/api/v1/auth/passkeys/registration/finish", {
    method: "POST",
    body: JSON.stringify({ credential: encodeRegistrationCredential(credential) }),
  });
}

export async function logoutOwnerSession(): Promise<void> {
  await apiRequest<unknown>("/api/v1/auth/passkey/logout", { method: "POST" });
}

export async function loadLanguagePreference(): Promise<LanguagePreference> {
  const payload = await apiRequest<LanguageSettings>("/api/v1/settings/language");
  return payload.preference;
}

export async function saveLanguagePreference(preference: LanguagePreference): Promise<LanguagePreference> {
  const payload = await apiRequest<LanguageSettings>("/api/v1/settings/language", {
    method: "PUT",
    body: JSON.stringify({ preference }),
  });
  return payload.preference;
}
