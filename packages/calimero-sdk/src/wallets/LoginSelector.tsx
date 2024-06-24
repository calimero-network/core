import React from 'react';
import { MetamaskIcon } from './MetamaskLogin/MetamaskIcon';
import { NearIcon } from './NearLogin/NearIcon';

export interface LoginSelectorProps {
  navigateMetamaskLogin: () => void | undefined;
  navigateNearLogin: () => void | undefined;
  cardBackgroundColor: string | undefined;
}

export const LoginSelector: React.FC<LoginSelectorProps> = ({
  navigateMetamaskLogin,
  navigateNearLogin,
  cardBackgroundColor,
}) => {
  return (
    <div
      style={{
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        justifyContent: 'center',
        backgroundColor: cardBackgroundColor ?? '#1C1C1C',
        gap: '1rem',
        borderRadius: '0.5rem',
        width: 'fit-content',
      }}
    >
      <div
        style={{
          padding: '2rem',
        }}
      >
        <div
          style={{
            width: '100%',
            textAlign: 'center',
            color: 'white',
            marginTop: '6px',
            marginBottom: '6px',
            fontSize: '1.5rem',
            lineHeight: '2rem',
            fontWeight: 'medium',
          }}
        >
          Continue with wallet
        </div>
        <div
          style={{
            display: 'flex',
            flexDirection: 'column',
            width: '100%',
            gap: '8px',
            paddingTop: '50px',
          }}
        >
          <button
            style={{
              width: '100%',
              display: 'flex',
              justifyContent: 'center',
              alignItems: 'center',
              gap: '2px',
              height: '46px',
              cursor: 'pointer',
              fontSize: '1rem',
              lineheight: '1.5rem',
              fontWeight: '500',
              lineHeight: '1.25rem',
              borderRadius: '0.375rem',
              backgroundColor: '#FF7A00',
              color: 'white',
              border: 'none',
              outline: 'none',
            }}
            onClick={navigateMetamaskLogin}
          >
            <MetamaskIcon />
            <span>Metamask wallet</span>
          </button>
          <button
            style={{
              width: '100%',
              display: 'flex',
              justifyContent: 'center',
              alignItems: 'center',
              gap: '2px',
              height: '46px',
              cursor: 'pointer',
              fontSize: '1rem',
              lineheight: '1.5rem',
              fontWeight: '500',
              lineHeight: '1.25rem',
              borderRadius: '0.375rem',
              backgroundColor: '#D1D5DB',
              color: 'black',
              border: 'none',
              outline: 'none',
            }}
            onClick={navigateNearLogin}
          >
            <NearIcon />
            <span>Near wallet</span>
          </button>
        </div>
      </div>
    </div>
  );
};
