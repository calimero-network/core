from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast
from typing import cast, Union

if TYPE_CHECKING:
    from ..models.access_type_3 import AccessType3
    from ..models.access_type_1 import AccessType1
    from ..models.access_type_4 import AccessType4
    from ..models.access_type_2 import AccessType2
    from ..models.access_type_0 import AccessType0


T = TypeVar("T", bound="AppAccessBody")


@_attrs_define
class AppAccessBody:
    """
    Attributes:
        access (Union['AccessType0', 'AccessType1', 'AccessType2', 'AccessType3', 'AccessType4']): Who may reach an app
            at all.
    """

    access: Union[
        "AccessType0", "AccessType1", "AccessType2", "AccessType3", "AccessType4"
    ]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.access_type_3 import AccessType3
        from ..models.access_type_1 import AccessType1
        from ..models.access_type_4 import AccessType4
        from ..models.access_type_2 import AccessType2
        from ..models.access_type_0 import AccessType0

        access: dict[str, Any]
        if isinstance(self.access, AccessType0):
            access = self.access.to_dict()
        elif isinstance(self.access, AccessType1):
            access = self.access.to_dict()
        elif isinstance(self.access, AccessType2):
            access = self.access.to_dict()
        elif isinstance(self.access, AccessType3):
            access = self.access.to_dict()
        else:
            access = self.access.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "access": access,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.access_type_3 import AccessType3
        from ..models.access_type_1 import AccessType1
        from ..models.access_type_4 import AccessType4
        from ..models.access_type_2 import AccessType2
        from ..models.access_type_0 import AccessType0

        d = dict(src_dict)

        def _parse_access(
            data: object,
        ) -> Union[
            "AccessType0", "AccessType1", "AccessType2", "AccessType3", "AccessType4"
        ]:
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_access_type_0 = AccessType0.from_dict(data)

                return componentsschemas_access_type_0
            except:  # noqa: E722
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_access_type_1 = AccessType1.from_dict(data)

                return componentsschemas_access_type_1
            except:  # noqa: E722
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_access_type_2 = AccessType2.from_dict(data)

                return componentsschemas_access_type_2
            except:  # noqa: E722
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_access_type_3 = AccessType3.from_dict(data)

                return componentsschemas_access_type_3
            except:  # noqa: E722
                pass
            if not isinstance(data, dict):
                raise TypeError()
            componentsschemas_access_type_4 = AccessType4.from_dict(data)

            return componentsschemas_access_type_4

        access = _parse_access(d.pop("access"))

        app_access_body = cls(
            access=access,
        )

        app_access_body.additional_properties = d
        return app_access_body

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
