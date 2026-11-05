import * as Schema from 'effect/Schema'

/** The C606's flash. Every op names flash only through `Region`, so a request past it dies at decode on both ends. */
export const MAX_FLASH_BYTES = 16 * 1024 * 1024

/**
 * One flasher hold on the page, minted by the agent per lease (16 chars,
 * base64url, 96 bits). An op whose lease is not the page's current one is
 * refused, so a restarted agent's reused request ids never reach someone
 * else's session.
 */
export const Lease = Schema.String.check(
  Schema.isPattern(/^[A-Za-z0-9_-]{16}$/),
).pipe(Schema.brand('Lease'))
export type Lease = typeof Lease.Type

export const Region = Schema.Struct({
  address: Schema.Int.check(
    Schema.isBetween({ minimum: 0, maximum: MAX_FLASH_BYTES - 1 }),
  ),
  size: Schema.Int.check(
    Schema.isBetween({ minimum: 1, maximum: MAX_FLASH_BYTES }),
  ),
}).check(
  Schema.makeFilter(
    region =>
      region.address + region.size <= MAX_FLASH_BYTES ||
      'region ends past flash',
  ),
)
export type Region = typeof Region.Type

/** Lowercase hex MD5, the digest the stub's SPI_FLASH_MD5 returns. */
export const Md5 = Schema.String.check(Schema.isPattern(/^[0-9a-f]{32}$/)).pipe(
  Schema.brand('Md5'),
)
export type Md5 = typeof Md5.Type

/**
 * The chip's eFuse MAC, 12 lowercase hex. It reaches the authenticated agent
 * (devtools checks backups against it) and nowhere else: never the page's
 * Model or view, never a log.
 */
export const Mac = Schema.String.check(Schema.isPattern(/^[0-9a-f]{12}$/)).pipe(
  Schema.brand('Mac'),
)
export type Mac = typeof Mac.Type

export const Chip = Schema.Struct({
  mac: Mac,
  /** GET_SECURITY_INFO's SECURE_BOOT_EN flag. */
  secureBoot: Schema.Boolean,
  /** Odd popcount of flash_crypt_cnt, as esptool computes it. */
  flashEncryption: Schema.Boolean,
})
export type Chip = typeof Chip.Type

/**
 * Everything an agent may ask of a visitor's device. There is no eFuse op,
 * no erase, no register access and no line toggling: what is not here cannot
 * be asked.
 */
export const Op = Schema.Union([
  /** Download mode, watchdogs off, stub running. Idempotent while held; another lease takes the hold over. */
  Schema.TaggedStruct('Enter', { lease: Lease }),
  /** The bytes follow as OpData frames toward the agent. */
  Schema.TaggedStruct('Read', { lease: Lease, region: Region }),
  /** `region.size` bytes follow as OpData frames toward the page; `md5` is theirs. */
  Schema.TaggedStruct('Write', { lease: Lease, region: Region, md5: Md5 }),
  Schema.TaggedStruct('Digest', { lease: Lease, region: Region }),
  /** Watchdog reset into the selected app; ends the hold. */
  Schema.TaggedStruct('Reboot', { lease: Lease }),
  /** Ends the hold. Any lease, any state: always answered Released. */
  Schema.TaggedStruct('Release', { lease: Lease }),
])
export type Op = typeof Op.Type
export type OpKind = Op['_tag']

export const OpFailure = Schema.Literals([
  /** The op's lease is not the page's current one, or nothing is held. */
  'notEntered',
  /** An op is already queued behind the running one; the agent sends one at a time. */
  'busy',
  /** The visitor declined the write. */
  'notAllowed',
  /** connect, sync or the stub failed, including one retry after a lost port. */
  'entryFailed',
  /** esptool-js failed during the op: a timeout, a bad reply, a flash error. */
  'chipError',
  /** Write payload frames were missing, out of order or past the region. */
  'transferGap',
  /** The write payload's MD5 differs from the announced one; flash untouched. */
  'payloadMismatch',
  /** The chip's digest after a write or read differs, after the page's own retries. */
  'digestMismatch',
  /** The USB device vanished mid-op: unplug, panic or an unexpected reset. */
  'portLost',
  /** A Release, the visitor or the end of the hold stopped the op. */
  'stopped',
])
export type OpFailure = typeof OpFailure.Type

/** At most this long: esptool-js's own words with MAC-shaped text redacted by the page. */
export const MAX_DETAIL_CHARS = 200

export const OpOutcome = Schema.Union([
  Schema.TaggedStruct('Entered', { chip: Chip }),
  /** The chip's digest of the whole region; the bytes went as OpData. */
  Schema.TaggedStruct('Read', { md5: Md5 }),
  /** The chip's digest of the region after writing it. */
  Schema.TaggedStruct('Written', { md5: Md5 }),
  Schema.TaggedStruct('Digest', { md5: Md5 }),
  /** Watchdog armed. The port is lost next and comes back as the console. */
  Schema.TaggedStruct('Rebooting', {}),
  Schema.TaggedStruct('Released', {}),
  Schema.TaggedStruct('Failed', {
    reason: OpFailure,
    detail: Schema.String.check(Schema.isMaxLength(MAX_DETAIL_CHARS)),
  }),
])
export type OpOutcome = typeof OpOutcome.Type

/** How often the page reports a running op, at the least; the agent calls an op stalled after `OP_STALL_MS` of silence. */
export const OP_PROGRESS_MS = 1000
export const OP_STALL_MS = 20_000
