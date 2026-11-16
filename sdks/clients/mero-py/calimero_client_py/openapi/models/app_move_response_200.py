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


T = TypeVar("T", bound="AppMoveResponse200")


@_attrs_define
class AppMoveResponse200:
    """
    Attributes:
        app_id (str):
        from_ (Union[None, str]):
        org (str):
        access (Union['AccessType0', 'AccessType1', 'AccessType2', 'AccessType3', 'AccessType4']): Who may reach an app
            at all.
        created (bool):
        added (list[str]):
        revoked_invites (int):
    """

    app_id: str
    from_: Union[None, str]
    org: str
    access: Union[
        "AccessType0", "AccessType1", "AccessType2", "AccessType3", "AccessType4"
    ]
    created: bool
    added: list[str]
    revoked_invites: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.access_type_3 import AccessType3
        from ..models.access_type_1 import AccessType1
        from ..models.access_type_4 import AccessType4
        from ..models.access_type_2 import AccessType2
        from ..models.access_type_0 import AccessType0

        app_id = self.app_id

        from_: Union[None, str]
        from_ = self.from_

        org = self.org

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

        created = self.created

        added = self.added

        revoked_invites = self.revoked_invites

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "appId": app_id,
                "from": from_,
                "org": org,
                "access": access,
                "created": created,
                "added": added,
                "revokedInvites": revoked_invites,
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
        app_id = d.pop("appId")

        def _parse_from_(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        from_ = _parse_from_(d.pop("from"))

        org = d.pop("org")

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

        created = d.pop("created")

        added = cast(list[str], d.pop("added"))

        revoked_invites = d.pop("revokedInvites")

        app_move_response_200 = cls(
            app_id=app_id,
            from_=from_,
            org=org,
            access=access,
            created=created,
            added=added,
            revoked_invites=revoked_invites,
        )

        app_move_response_200.additional_properties = d
        return app_move_response_200

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
