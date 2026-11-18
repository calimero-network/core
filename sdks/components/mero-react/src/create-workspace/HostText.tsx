import { Fragment } from 'react';

const MAX_UNBROKEN = 30; // a longer piece may still break anywhere, or it would overflow

/** A host or id that wraps only after its dots: each dotted piece stays whole, hyphens included. */
export function HostText({ text }: { text: string }) {
  return (
    <>
      {text.split('.').map((piece, i, all) => (
        <Fragment key={i}>
          {/^\S+$/.test(piece) && piece.length <= MAX_UNBROKEN ? <span className="mero-cw-review__nobreak">{piece}</span> : piece}
          {i < all.length - 1 && (
            <>
              .
              <wbr />
            </>
          )}
        </Fragment>
      ))}
    </>
  );
}
