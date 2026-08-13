export interface LcuCredentials {
  installPath: string;
  processName: string;
  processId: number;
  port: number;
  password: string;
  protocol: 'https';
}

export interface LcuJsonApiEvent {
  uri: string;
  eventType: string;
  data: unknown;
}

export interface LcuEventConnectionState {
  connected: boolean;
  connecting: boolean;
  message: string;
}

export function lcuCredentialSignature(credentials: LcuCredentials): string {
  return `${credentials.processId}:${credentials.port}:${credentials.password}`;
}
