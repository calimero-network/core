from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.update_reason_type_4_code import UpdateReasonType4Code
from typing import cast

T = TypeVar("T", bound="UpdateReasonType4")


@_attrs_define
class UpdateReasonType4:
    """
    Attributes:
        code (UpdateReasonType4Code):
        member (str):
        package (str):
        egress (list[str]):
    """

    code: UpdateReasonType4Code
    member: str
    package: str
    egress: list[str]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        code = self.code.value

        member = self.member

        package = self.package

        egress = self.egress

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "code": code,
                "member": member,
                "package": package,
                "egress": egress,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        code = UpdateReasonType4Code(d.pop("code"))

        member = d.pop("member")

        package = d.pop("package")

        egress = cast(list[str], d.pop("egress"))

        update_reason_type_4 = cls(
            code=code,
            member=member,
            package=package,
            egress=egress,
        )

        update_reason_type_4.additional_properties = d
        return update_reason_type_4

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
