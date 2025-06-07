// Safe SHA256 implementation without eval
import { createHash } from 'crypto-browserify';

export function sha256(input) {
  return createHash('sha256').update(input).digest('hex');
}

export default {
  sha256,
  hmac: function(key, data) {
    return createHash('sha256').update(key).update(data).digest('hex');
  }
}; 