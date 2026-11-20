from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.update_reason_type_0_code import UpdateReasonType0Code
from typing import cast, Union

T = TypeVar("T", bound="UpdateReasonType0")


@_attrs_define
class UpdateReasonType0:
    """
    Attributes:
        code (UpdateReasonType0Code):
        member (Union[None, str]):
        from_ (str):
        to (str):
    """

    code: UpdateReasonType0Code
    member: Union[None, str]
    from_: str
    to: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        code = self.code.value

        member: Union[None, str]
        member = self.member

        from_ = self.from_

        to = self.to

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "code": code,
                "member": member,
                "from": from_,
                "to": to,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        code = UpdateReasonType0Code(d.pop("code"))

        def _parse_member(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        member = _parse_member(d.pop("member"))

        from_ = d.pop("from")

        to = d.pop("to")

        update_reason_type_0 = cls(
            code=code,
            member=member,
            from_=from_,
            to=to,
        )

        update_reason_type_0.additional_properties = d
        return update_reason_type_0

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
