import { invoke } from "@tauri-apps/api/core";
import { auth } from "./firebase";
import {
  GoogleAuthProvider,
  signInWithCredential,
  signOut as firebaseSignOut,
  onAuthStateChanged,
  User,
} from "firebase/auth";

export function initAuth(): Promise<User | null> {
  return new Promise((resolve) => {
    const unsubscribe = onAuthStateChanged(auth, (user) => {
      unsubscribe();
      resolve(user);
    });
  });
}

export async function signIn(): Promise<User> {
  // Open Google OAuth in system browser, get tokens via local HTTP callback
  const tokens: { id_token: string; access_token: string } =
    await invoke("google_sign_in");

  const credential = GoogleAuthProvider.credential(
    tokens.id_token,
    tokens.access_token
  );
  const result = await signInWithCredential(auth, credential);
  return result.user;
}

export async function signOut(): Promise<void> {
  await firebaseSignOut(auth);
}

export function onAuthChanged(callback: (user: User | null) => void) {
  return onAuthStateChanged(auth, callback);
}
