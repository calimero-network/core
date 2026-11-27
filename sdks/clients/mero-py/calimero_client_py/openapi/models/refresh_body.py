from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.refresh_body_holder_proof import RefreshBodyHolderProof


T = TypeVar("T", bound="RefreshBody")


@_attrs_define
class RefreshBody:
    """
    Attributes:
        refresh_token (str):
        holder_proof (Union[Unset, RefreshBodyHolderProof]):
    """

    refresh_token: str
    holder_proof: Union[Unset, "RefreshBodyHolderProof"] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.refresh_body_holder_proof import RefreshBodyHolderProof

        refresh_token = self.refresh_token

        holder_proof: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.holder_proof, Unset):
            holder_proof = self.holder_proof.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "refreshToken": refresh_token,
            }
        )
        if holder_proof is not UNSET:
            field_dict["holderProof"] = holder_proof

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.refresh_body_holder_proof import RefreshBodyHolderProof

        d = dict(src_dict)
        refresh_token = d.pop("refreshToken")

        _holder_proof = d.pop("holderProof", UNSET)
        holder_proof: Union[Unset, RefreshBodyHolderProof]
        if isinstance(_holder_proof, Unset):
            holder_proof = UNSET
        else:
            holder_proof = RefreshBodyHolderProof.from_dict(_holder_proof)

        refresh_body = cls(
            refresh_token=refresh_token,
            holder_proof=holder_proof,
        )

        refresh_body.additional_properties = d
        return refresh_body

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> Any:
        return self.additional_properties[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
