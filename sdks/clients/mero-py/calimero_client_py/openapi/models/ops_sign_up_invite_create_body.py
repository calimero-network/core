from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.operator_proof import OperatorProof


T = TypeVar("T", bound="OpsSignUpInviteCreateBody")


@_attrs_define
class OpsSignUpInviteCreateBody:
    """
    Attributes:
        token (str):
        expires_at (str):
        uses (Union[Unset, int]):
        operator_proof (Union[Unset, OperatorProof]): The operator key's Ed25519 signature (hex) over the statement a
            `428 OPERATOR_SIGNATURE_REQUIRED` named, with that statement's nonce.
    """

    token: str
    expires_at: str
    uses: Union[Unset, int] = UNSET
    operator_proof: Union[Unset, "OperatorProof"] = UNSET

    def to_dict(self) -> dict[str, Any]:
        from ..models.operator_proof import OperatorProof

        token = self.token

        expires_at = self.expires_at

        uses = self.uses

        operator_proof: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.operator_proof, Unset):
            operator_proof = self.operator_proof.to_dict()

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "token": token,
                "expiresAt": expires_at,
            }
        )
        if uses is not UNSET:
            field_dict["uses"] = uses
        if operator_proof is not UNSET:
            field_dict["operatorProof"] = operator_proof

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.operator_proof import OperatorProof

        d = dict(src_dict)
        token = d.pop("token")

        expires_at = d.pop("expiresAt")

        uses = d.pop("uses", UNSET)

        _operator_proof = d.pop("operatorProof", UNSET)
        operator_proof: Union[Unset, OperatorProof]
        if isinstance(_operator_proof, Unset):
            operator_proof = UNSET
        else:
            operator_proof = OperatorProof.from_dict(_operator_proof)

        ops_sign_up_invite_create_body = cls(
            token=token,
            expires_at=expires_at,
            uses=uses,
            operator_proof=operator_proof,
        )

        return ops_sign_up_invite_create_body
