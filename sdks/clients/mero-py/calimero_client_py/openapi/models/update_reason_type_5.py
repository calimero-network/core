from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.update_reason_type_5_code import UpdateReasonType5Code
from typing import cast
from typing import cast, Union

T = TypeVar("T", bound="UpdateReasonType5")


@_attrs_define
class UpdateReasonType5:
    """
    Attributes:
        code (UpdateReasonType5Code):
        member (Union[None, str]):
        package (str):
        methods (list[str]):
    """

    code: UpdateReasonType5Code
    member: Union[None, str]
    package: str
    methods: list[str]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        code = self.code.value

        member: Union[None, str]
        member = self.member

        package = self.package

        methods = self.methods

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "code": code,
                "member": member,
                "package": package,
                "methods": methods,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        code = UpdateReasonType5Code(d.pop("code"))

        def _parse_member(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        member = _parse_member(d.pop("member"))

        package = d.pop("package")

        methods = cast(list[str], d.pop("methods"))

        update_reason_type_5 = cls(
            code=code,
            member=member,
            package=package,
            methods=methods,
        )

        update_reason_type_5.additional_properties = d
        return update_reason_type_5

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
