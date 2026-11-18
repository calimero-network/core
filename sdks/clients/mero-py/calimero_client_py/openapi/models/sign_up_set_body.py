from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.sign_up import SignUp
from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.operator_proof import OperatorProof
    from ..models.confirm import Confirm


T = TypeVar("T", bound="SignUpSetBody")


@_attrs_define
class SignUpSetBody:
    """
    Attributes:
        policy (SignUp): The cluster's sign-up policy.
        operator_proof (Union[Unset, OperatorProof]): The operator key's Ed25519 signature (hex) over the statement a
            `428 OPERATOR_SIGNATURE_REQUIRED` named, with that statement's nonce.
        confirm (Union[Unset, Confirm]): A passkey assertion over the challenge a `412 CONFIRM_REQUIRED` named: the same
            request sent again with it approves exactly that request.
    """

    policy: SignUp
    operator_proof: Union[Unset, "OperatorProof"] = UNSET
    confirm: Union[Unset, "Confirm"] = UNSET

    def to_dict(self) -> dict[str, Any]:
        from ..models.operator_proof import OperatorProof
        from ..models.confirm import Confirm

        policy = self.policy.value

        operator_proof: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.operator_proof, Unset):
            operator_proof = self.operator_proof.to_dict()

        confirm: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.confirm, Unset):
            confirm = self.confirm.to_dict()

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "policy": policy,
            }
        )
        if operator_proof is not UNSET:
            field_dict["operatorProof"] = operator_proof
        if confirm is not UNSET:
            field_dict["confirm"] = confirm

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.operator_proof import OperatorProof
        from ..models.confirm import Confirm

        d = dict(src_dict)
        policy = SignUp(d.pop("policy"))

        _operator_proof = d.pop("operatorProof", UNSET)
        operator_proof: Union[Unset, OperatorProof]
        if isinstance(_operator_proof, Unset):
            operator_proof = UNSET
        else:
            operator_proof = OperatorProof.from_dict(_operator_proof)

        _confirm = d.pop("confirm", UNSET)
        confirm: Union[Unset, Confirm]
        if isinstance(_confirm, Unset):
            confirm = UNSET
        else:
            confirm = Confirm.from_dict(_confirm)

        sign_up_set_body = cls(
            policy=policy,
            operator_proof=operator_proof,
            confirm=confirm,
        )

        return sign_up_set_body
