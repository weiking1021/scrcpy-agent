// Fails to compile when `Host` (index.d.ts) and the list of the program (`HostMember`, written from Rust) differ: a
// function in one and not in the other makes `same` a type that `true` does not fit.

import type { Host, HostMember } from "../index";

type Paths<T> = {
  [K in keyof T & string]: T[K] extends (...args: never[]) => unknown ? K : `${K}.${keyof T[K] & string}`;
}[keyof T & string];

type Equal<A, B> = [A] extends [B] ? ([B] extends [A] ? true : never) : never;

export const same: Equal<Paths<Host>, HostMember> = true;
