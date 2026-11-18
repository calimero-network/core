import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { ConnectButton } from '../components';
import { MockMeroProvider } from '../test-utils/MockMeroProvider';

describe('ConnectButton / LoginModal', () => {
  it('on a node-served page asks only for the passkey', async () => {
    const signIn = vi.fn(async () => null);
    render(
      <MockMeroProvider servedByNode signIn={signIn}>
        <ConnectButton appName="KV" />
      </MockMeroProvider>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Sign in' }));
    expect(screen.getByRole('dialog')).toBeTruthy();
    expect(screen.queryByLabelText('Node URL or cluster')).toBeNull();
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Sign in with passkey' }));
    });
    expect(signIn).toHaveBeenCalled();
  });

  it('elsewhere asks for the node, and a name to create an account', async () => {
    const signUp = vi.fn(async () => null);
    const connectToNode = vi.fn();
    render(
      <MockMeroProvider signUp={signUp} connectToNode={connectToNode} nodeUrl={null}>
        <ConnectButton />
      </MockMeroProvider>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Sign in' }));
    fireEvent.click(screen.getByRole('button', { name: 'New here? Create an account' }));
    fireEvent.click(screen.getByRole('button', { name: 'Create account' }));
    expect(screen.getByRole('alert').textContent).toMatch(/name/);
    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'ada' } });
    fireEvent.change(screen.getByLabelText('Node URL or cluster'), { target: { value: 'not a url' } });
    fireEvent.click(screen.getByRole('button', { name: 'Create account' }));
    expect(screen.getByRole('alert').textContent).toMatch(/node URL/);
    fireEvent.change(screen.getByLabelText('Node URL or cluster'), { target: { value: 'https://n.test' } });
    fireEvent.click(screen.getByRole('button', { name: 'Create account' }));
    expect(connectToNode).toHaveBeenCalledWith('https://n.test');
  });

  it('takes a cluster name in place of a node URL', async () => {
    const connectToCluster = vi.fn();
    const connectToNode = vi.fn();
    render(
      <MockMeroProvider connectToCluster={connectToCluster} connectToNode={connectToNode} nodeUrl={null}>
        <ConnectButton />
      </MockMeroProvider>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Sign in' }));
    fireEvent.change(screen.getByLabelText('Node URL or cluster'), { target: { value: ' eu-shared-1 ' } });
    fireEvent.click(screen.getByRole('button', { name: 'Sign in with passkey' }));
    expect(connectToCluster).toHaveBeenCalledWith('eu-shared-1');
    expect(connectToNode).not.toHaveBeenCalled();
  });

  it('shows the account and signs out when signed in', async () => {
    const signOut = vi.fn(async () => undefined);
    render(
      <MockMeroProvider isAuthenticated accountId="acct_0123456789abcdef" signOut={signOut}>
        <ConnectButton />
      </MockMeroProvider>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'acct_0…abcdef' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Sign out' }));
    await waitFor(() => expect(signOut).toHaveBeenCalled());
  });
});
