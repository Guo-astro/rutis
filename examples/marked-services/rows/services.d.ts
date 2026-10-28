// Generated from the service declarations; do not edit.

/** What a method's `Err(e)` throws: named after `e`'s type, `code` its variant, `message` its `Display`, `data` its serde value. */
export interface RustError<Name extends string = string, Code extends string = string, Data = unknown> extends Error {
  readonly name: Name
  readonly code: Code
  readonly data: Data
}

/** A calendar entry. */
export interface Entry {
  title: string
  /** Milliseconds since the Unix epoch. */
  startsAt: number | bigint
}

export type CalendarError = { NotFound: { id: number | bigint } }

/** How often a planned entry repeats. */
export type Repeat = "never" | "daily" | "weekly"

/** A reminder before a planned entry. */
export interface Reminder {
  minutesBefore: number
  /** Off when left out. */
  sound?: boolean
  note?: string | null
}

/** The application's calendar. */
export interface Calendar {
  /** Add an entry; its id. */
  add(entry: Entry): Promise<number | bigint>
  /**
   * The entry `id`.
   * @throws {CalendarErrorThrown}
   */
  get(id: number | bigint): Promise<Entry>
  /**
   * Call `visit` with each entry, in order; how many there were.
   * @throws {CordisErrorThrown}
   */
  each(visit: (arg0: Entry) => void | Promise<void>): Promise<number | bigint>
  /**
   * Plan an entry; its number. Its parameters take one of each kind of
   * value a service is given, so that the generated declarations show
   * each.
   */
  plan(title: string, allDay: boolean, hours: number, repeat: Repeat, reminders: Reminder[], window: [number, number] | null, labels: Record<string, string>): Promise<number | bigint>
  /**
   * Add a plugin that brings entries from elsewhere, by name; how many
   * there are.
   */
  addSource(name: string, source: import('@arcships/rutis').Plugin | ((ctx: any, config: any) => unknown) | { apply(ctx: any, config: any): unknown }): Promise<number | bigint>
}

/** What a method returning `Err(CalendarError)` throws. */
export type CalendarErrorThrown = RustError<"CalendarError", "NotFound", CalendarError>

/** What a method returning `Err(CordisError)` throws. */
export type CordisErrorThrown = RustError<"CordisError", string, undefined>

/** The exported services by name: `ctx.use<Services['name']>('name')`. */
export interface Services {
  calendar: Calendar
}
