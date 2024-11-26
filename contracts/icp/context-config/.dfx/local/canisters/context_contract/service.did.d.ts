import type { Principal } from '@dfinity/principal';
import type { ActorMethod } from '@dfinity/agent';
import type { IDL } from '@dfinity/candid';

export interface Context {
  'members' : GuardedMembers,
  'application' : GuardedApplication,
  'proxy' : GuardedProxy,
}
export interface GuardedApplication {
  'privileged' : Array<Uint8Array | number[]>,
  'inner' : Uint8Array | number[],
  'revision' : bigint,
}
export interface GuardedMembers {
  'privileged' : Array<Uint8Array | number[]>,
  'inner' : Array<Uint8Array | number[]>,
  'revision' : bigint,
}
export interface GuardedProxy {
  'privileged' : Array<Uint8Array | number[]>,
  'inner' : Principal,
  'revision' : bigint,
}
export interface Request {
  'context_id' : Uint8Array | number[],
  'kind' : RequestKind,
  'signer_id' : Uint8Array | number[],
}
export type RequestKind = {
    'Add' : {
      'application' : Uint8Array | number[],
      'author_id' : Uint8Array | number[],
    }
  };
export interface _SERVICE { 'mutate' : ActorMethod<[Request], undefined> }
export declare const idlFactory: IDL.InterfaceFactory;
export declare const init: (args: { IDL: typeof IDL }) => IDL.Type[];
