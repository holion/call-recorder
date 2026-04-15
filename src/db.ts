import { db } from "./firebase";
import {
  collection,
  doc,
  setDoc,
  deleteDoc,
  onSnapshot,
  query,
  orderBy,
  Timestamp,
  Unsubscribe,
} from "firebase/firestore";

export interface RecordingMeta {
  id: string;
  title: string;
  created_at: Timestamp;
  duration_secs: number;
  has_system_audio: boolean;
  transcription: string | null;
  transcription_status: "NotStarted" | "InProgress" | "Done" | "Failed";
}

function recordingsRef(uid: string) {
  return collection(db, "users", uid, "recordings");
}

export function subscribeToRecordings(
  uid: string,
  callback: (recordings: RecordingMeta[]) => void
): Unsubscribe {
  const q = query(recordingsRef(uid), orderBy("created_at", "desc"));
  return onSnapshot(q, (snapshot) => {
    const recordings = snapshot.docs.map((doc) => ({
      id: doc.id,
      ...doc.data(),
    })) as RecordingMeta[];
    callback(recordings);
  });
}

export async function saveRecording(
  uid: string,
  meta: RecordingMeta
): Promise<void> {
  const ref = doc(recordingsRef(uid), meta.id);
  await setDoc(ref, {
    title: meta.title,
    created_at: meta.created_at,
    duration_secs: meta.duration_secs,
    has_system_audio: meta.has_system_audio,
    transcription: meta.transcription,
    transcription_status: meta.transcription_status,
  });
}

export async function updateRecording(
  uid: string,
  id: string,
  fields: Partial<Omit<RecordingMeta, "id">>
): Promise<void> {
  const ref = doc(recordingsRef(uid), id);
  await setDoc(ref, fields, { merge: true });
}

export async function deleteRecordingDoc(
  uid: string,
  id: string
): Promise<void> {
  const ref = doc(recordingsRef(uid), id);
  await deleteDoc(ref);
}

export interface StatusControl {
  action: "idle" | "record" | "stop";
}

export function subscribeToControl(
  uid: string,
  callback: (control: StatusControl) => void
): Unsubscribe {
  const ref = doc(db, "users", uid, "status", "control");
  return onSnapshot(ref, (snapshot) => {
    const data = snapshot.data() as StatusControl | undefined;
    if (data) {
      callback(data);
    }
  });
}

export async function resetControl(uid: string): Promise<void> {
  const ref = doc(db, "users", uid, "status", "control");
  await setDoc(ref, { action: "idle" });
}
