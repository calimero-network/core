from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.outbound_approval import OutboundApproval


T = TypeVar("T", bound="OutboundApproved")


@_attrs_define
class OutboundApproved:
    """The outbound approval the scope holds now for the package's publisher (`signer`), after the change.

    Attributes:
        signer (str):
        approval (OutboundApproval): What an owner approved for one publisher's apps to call out to: the approved URL
            prefixes, and every prefix it was shown.
    """

    signer: str
    approval: "OutboundApproval"
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.outbound_approval import OutboundApproval

        signer = self.signer

        approval = self.approval.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "signer": signer,
                "approval": approval,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.outbound_approval import OutboundApproval

        d = dict(src_dict)
        signer = d.pop("signer")

        approval = OutboundApproval.from_dict(d.pop("approval"))

        outbound_approved = cls(
            signer=signer,
            approval=approval,
        )

        outbound_approved.additional_properties = d
        return outbound_approved

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
