import {access, readdir, readFile, stat} from 'node:fs/promises';
import {join, normalize, resolve} from 'node:path';
import type {LcuCredentials} from './types';

const MAX_METADATA_BYTES = 256 * 1024;
const MAX_LOCKFILE_BYTES = 2048;

export interface ClientLocatorOptions {
  metadataRoot?: string;
  fallbackPaths?: string[];
}

export interface LcuClientLocator {
  findRunningClient(): Promise<LcuCredentials | null>;
}

export class ClientLocator implements LcuClientLocator {
  constructor(private readonly options: ClientLocatorOptions = {}) {
  }

  public async findRunningClient(): Promise<LcuCredentials | null> {
    const installPaths = await this.discoverInstallPaths();
    for (const installPath of installPaths) {
      const lockfilePath = join(installPath, 'lockfile');
      try {
        if ((await stat(lockfilePath)).size > MAX_LOCKFILE_BYTES) continue;
        const credentials = parseLockfile(await readFile(lockfilePath, 'utf8'), installPath);
        if (credentials) return credentials;
      } catch {
        continue;
      }
    }
    return null;
  }

  private async discoverInstallPaths(): Promise<string[]> {
    const paths = [
      ...await this.metadataInstallPaths(),
      ...(this.options.fallbackPaths ?? this.defaultFallbackPaths())
    ];
    const output: string[] = [];
    const seen = new Set<string>();
    for (const candidate of paths) {
      const path = normalize(String(candidate || '').trim().replace(/^"|"$/g, ''));
      const key = this.pathKey(path);
      if (!key || seen.has(key) || !await this.isLeagueInstall(path)) continue;
      seen.add(key);
      output.push(path);
    }
    return output;
  }

  private async metadataInstallPaths(): Promise<string[]> {
    const root = this.options.metadataRoot ?? this.defaultMetadataRoot();
    const paths: string[] = [];
    try {
      const entries = await readdir(root, {withFileTypes: true});
      for (const entry of entries) {
        if (!entry.isDirectory() || !/^league_of_legends\.[a-z0-9_]+$/i.test(entry.name)) continue;
        const settingsPath = join(root, entry.name, `${entry.name}.product_settings.yaml`);
        try {
          if ((await stat(settingsPath)).size > MAX_METADATA_BYTES) continue;
          const contents = await readFile(settingsPath, 'utf8');
          const match = /^\s*product_install_full_path:\s*"?([^"\r\n]+?)"?\s*$/m.exec(contents);
          if (match?.[1]) paths.push(match[1]);
        } catch {
          continue;
        }
      }
    } catch {
      return paths;
    }
    return paths;
  }

  private async isLeagueInstall(installPath: string): Promise<boolean> {
    if (!installPath || installPath.length >= 4096) return false;
    return await this.exists(join(installPath, 'LeagueClient.exe'))
      || await this.exists(join(installPath, 'LeagueClientUx.exe'))
      || await this.exists(join(installPath, 'lockfile'));
  }

  private async exists(path: string): Promise<boolean> {
    try {
      await access(path);
      return true;
    } catch {
      return false;
    }
  }

  private defaultMetadataRoot(): string {
    const programData = process.env['ProgramData'] || process.env['ALLUSERSPROFILE'] || 'C:\\ProgramData';
    return join(programData, 'Riot Games', 'Metadata');
  }

  private defaultFallbackPaths(): string[] {
    const systemDrive = process.env['SystemDrive'] || 'C:';
    return [join(`${systemDrive}\\`, 'Riot Games', 'League of Legends')];
  }

  private pathKey(path: string): string {
    try {
      return resolve(path).toLowerCase();
    } catch {
      return '';
    }
  }
}

export function parseLockfile(contents: string, installPath: string): LcuCredentials | null {
  const parts = String(contents || '').trim().split(':');
  if (parts.length !== 5) return null;
  const [processName = '', processIdText = '', portText = '', password = '', protocol = ''] = parts;
  const processId = Number(processIdText);
  const port = Number(portText);
  if (!/^[A-Za-z0-9._-]{1,128}$/.test(processName)) return null;
  if (!Number.isSafeInteger(processId) || processId <= 0) return null;
  if (!Number.isInteger(port) || port < 1 || port > 65535) return null;
  if (!password || password.length > 512 || containsControlCharacters(password)) return null;
  if (protocol.toLowerCase() !== 'https') return null;
  return {
    installPath: normalize(installPath),
    processName,
    processId,
    port,
    password,
    protocol: 'https'
  };
}

function containsControlCharacters(value: string): boolean {
  return Array.from(value).some(character => {
    const code = character.charCodeAt(0);
    return code <= 31 || code === 127;
  });
}
