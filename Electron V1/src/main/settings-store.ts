import {mkdir, readFile, rename, writeFile} from 'node:fs/promises';
import {dirname, join} from 'node:path';
import type {AppSettings} from '../shared/contracts';
import {DEFAULT_SETTINGS, mergeSettings, normalizeSettings} from '../shared/settings';

export interface StoredSecret {
  protection: 'os' | 'plain';
  value: string;
}

export interface SecretCodec {
  encode(value: string): StoredSecret;
  decode(value: StoredSecret): string;
}

interface StoredSettings extends Omit<AppSettings, 'discordWebhookUrl'> {
  discordWebhookUrl: StoredSecret;
}

export class SettingsStore {
  private readonly settingsPath: string;
  private settings: AppSettings = structuredClone(DEFAULT_SETTINGS);

  constructor(
    userDataDirectory: string,
    private readonly secretCodec: SecretCodec
  ) {
    this.settingsPath = join(userDataDirectory, 'settings.json');
  }

  public async load(): Promise<AppSettings> {
    try {
      const raw = JSON.parse(await readFile(this.settingsPath, 'utf8')) as Record<string, unknown>;
      const encoded = this.storedSecret(raw.discordWebhookUrl);
      this.settings = normalizeSettings({
        ...raw,
        discordWebhookUrl: encoded ? this.secretCodec.decode(encoded) : ''
      });
    } catch {
      this.settings = structuredClone(DEFAULT_SETTINGS);
      await this.save();
    }
    return this.snapshot();
  }

  public async update(patch: unknown): Promise<AppSettings> {
    this.settings = mergeSettings(this.settings, patch);
    await this.save();
    return this.snapshot();
  }

  public snapshot(): AppSettings {
    return structuredClone(this.settings);
  }

  private async save(): Promise<void> {
    const stored: StoredSettings = {
      ...this.settings,
      discordWebhookUrl: this.secretCodec.encode(this.settings.discordWebhookUrl)
    };
    const temporaryPath = `${this.settingsPath}.tmp`;
    await mkdir(dirname(this.settingsPath), {recursive: true});
    await writeFile(temporaryPath, `${JSON.stringify(stored, null, 2)}\n`, 'utf8');
    await rename(temporaryPath, this.settingsPath);
  }

  private storedSecret(value: unknown): StoredSecret | null {
    if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
    const candidate = value as Record<string, unknown>;
    if ((candidate.protection !== 'os' && candidate.protection !== 'plain') || typeof candidate.value !== 'string') {
      return null;
    }
    return {protection: candidate.protection, value: candidate.value};
  }
}
