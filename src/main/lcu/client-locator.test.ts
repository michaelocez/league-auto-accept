import assert from 'node:assert/strict';
import {mkdir, mkdtemp, rm, writeFile} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {afterEach, describe, it} from 'node:test';
import {ClientLocator, parseLockfile} from './client-locator';

const testDirectories: string[] = [];

async function testDirectory(): Promise<string> {
  const value = await mkdtemp(join(tmpdir(), 'league-client-locator-'));
  testDirectories.push(value);
  return value;
}

afterEach(async () => {
  await Promise.all(testDirectories.splice(0).map(value => rm(value, {recursive: true, force: true})));
});
describe('ClientLocator', () => {
  it('discovers and parses a running League installation from Riot metadata', async () => {
    const root = await testDirectory();
    const metadataRoot = join(root, 'Metadata');
    const product = 'league_of_legends.live';
    const productRoot = join(metadataRoot, product);
    const installPath = join(root, 'League of Legends');
    await mkdir(productRoot, {recursive: true});
    await mkdir(installPath, {recursive: true});
    await writeFile(join(installPath, 'LeagueClient.exe'), '', 'utf8');
    await writeFile(join(installPath, 'lockfile'), 'LeagueClientUx:1234:54321:local-secret:https', 'utf8');
    await writeFile(
      join(productRoot, `${product}.product_settings.yaml`),
      `product_install_full_path: "${installPath}"\n`,
      'utf8'
    );

    const credentials = await new ClientLocator({metadataRoot, fallbackPaths: []}).findRunningClient();
    assert.ok(credentials);
    assert.equal(credentials.port, 54321);
    assert.equal(credentials.password, 'local-secret');
    assert.equal(credentials.installPath, installPath);
  });

  it('rejects malformed or non-HTTPS lockfiles', () => {
    assert.equal(parseLockfile('LeagueClientUx:1234:70000:secret:https', 'C:\\League'), null);
    assert.equal(parseLockfile('LeagueClientUx:1234:54321:secret:http', 'C:\\League'), null);
    assert.equal(parseLockfile('LeagueClientUx:not-a-pid:54321:secret:https', 'C:\\League'), null);
  });
});
