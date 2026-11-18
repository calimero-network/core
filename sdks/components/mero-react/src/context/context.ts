import { createContext } from 'react';
import type { MeroContextValue } from '../types';

// Its own module so the node-shield entry shares this one context object with the main entry.
export const MeroContext = createContext<MeroContextValue | null>(null);
