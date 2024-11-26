export const idlFactory = ({ IDL }) => {
  const RequestKind = IDL.Variant({
    'Add' : IDL.Record({
      'application' : IDL.Vec(IDL.Nat8),
      'author_id' : IDL.Vec(IDL.Nat8),
    }),
  });
  const Request = IDL.Record({
    'context_id' : IDL.Vec(IDL.Nat8),
    'kind' : RequestKind,
    'signer_id' : IDL.Vec(IDL.Nat8),
  });
  return IDL.Service({ 'mutate' : IDL.Func([Request], [], []) });
};
export const init = ({ IDL }) => { return []; };
