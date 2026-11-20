from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.update_reason_type_2_code import UpdateReasonType2Code
from typing import cast, Union

T = TypeVar("T", bound="UpdateReasonType2")


@_attrs_define
class UpdateReasonType2:
    """
    Attributes:
        code (UpdateReasonType2Code):
        member (Union[None, str]):
        from_ (Union[None, int]):
        to (Union[None, int]):
    """

    code: UpdateReasonType2Code
    member: Union[None, str]
    from_: Union[None, int]
    to: Union[None, int]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        code = self.code.value

        member: Union[None, str]
        member = self.member

        from_: Union[None, int]
        from_ = self.from_

        to: Union[None, int]
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
        code = UpdateReasonType2Code(d.pop("code"))

        def _parse_member(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        member = _parse_member(d.pop("member"))

        def _parse_from_(data: object) -> Union[None, int]:
            if data is None:
                return data
            return cast(Union[None, int], data)

        from_ = _parse_from_(d.pop("from"))

        def _parse_to(data: object) -> Union[None, int]:
            if data is None:
                return data
            return cast(Union[None, int], data)

        to = _parse_to(d.pop("to"))

        update_reason_type_2 = cls(
            code=code,
            member=member,
            from_=from_,
            to=to,
        )

        update_reason_type_2.additional_properties = d
        return update_reason_type_2

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
