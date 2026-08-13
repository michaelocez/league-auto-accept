import {safeStorage} from 'electron';
import type {SecretCodec, StoredSecret} from './settings-store';

export class ElectronSecretCodec implements SecretCodec {
  public encode(value: string): StoredSecret {
    if (!value) return {protection: 'os', value: ''};
    if (!safeStorage.isEncryptionAvailable()) {
      return {protection: 'plain', value};
    }
    return {
      protection: 'os',
      value: safeStorage.encryptString(value).toString('base64')
    };
  }

  public decode(secret: StoredSecret): string {
    if (!secret.value) return '';
    if (secret.protection === 'plain') return secret.value;
    if (!safeStorage.isEncryptionAvailable()) return '';
    try {
      return safeStorage.decryptString(Buffer.from(secret.value, 'base64'));
    } catch {
      return '';
    }
  }
}
