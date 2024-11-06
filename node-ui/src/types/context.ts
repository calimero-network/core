import { Package, Release } from '../pages/Applications';

export interface ContextDetails {
  contextId: string;
  applicationId: string;
  package: Package | null;
  release: Release | null;
}

export interface Invitation {
  id: string;
  invitedOn: string;
}

export interface ContextObject {
  id: string;
  package: Package | null;
}

export interface ContextsList {
  joined: ContextObject[];
}
