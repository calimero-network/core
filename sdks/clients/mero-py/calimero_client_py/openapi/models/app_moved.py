from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast
from typing import cast, Union

if TYPE_CHECKING:
    from ..models.grant_type_0 import GrantType0
    from ..models.grant_type_1 import GrantType1
    from ..models.grant_type_3 import GrantType3
    from ..models.grant_type_2 import GrantType2
    from ..models.grant_type_4 import GrantType4


T = TypeVar("T", bound="AppMoved")


@_attrs_define
class AppMoved:
    """What `POST /apps/{app}/move` answers: where the app went, its access there, whether an app team was made, the
    accounts brought in and the invites revoked.

        Attributes:
            app_id (str):
            from_ (str):
            org (str):
            grants (list[Union['GrantType0', 'GrantType1', 'GrantType2', 'GrantType3', 'GrantType4']]):
            created (bool):
            added (list[str]):
            revoked_invites (int):
    """

    app_id: str
    from_: str
    org: str
    grants: list[
        Union["GrantType0", "GrantType1", "GrantType2", "GrantType3", "GrantType4"]
    ]
    created: bool
    added: list[str]
    revoked_invites: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.grant_type_0 import GrantType0
        from ..models.grant_type_1 import GrantType1
        from ..models.grant_type_3 import GrantType3
        from ..models.grant_type_2 import GrantType2
        from ..models.grant_type_4 import GrantType4

        app_id = self.app_id

        from_ = self.from_

        org = self.org

        grants = []
        for grants_item_data in self.grants:
            grants_item: dict[str, Any]
            if isinstance(grants_item_data, GrantType0):
                grants_item = grants_item_data.to_dict()
            elif isinstance(grants_item_data, GrantType1):
                grants_item = grants_item_data.to_dict()
            elif isinstance(grants_item_data, GrantType2):
                grants_item = grants_item_data.to_dict()
            elif isinstance(grants_item_data, GrantType3):
                grants_item = grants_item_data.to_dict()
            else:
                grants_item = grants_item_data.to_dict()

            grants.append(grants_item)

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
                "grants": grants,
                "created": created,
                "added": added,
                "revokedInvites": revoked_invites,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.grant_type_0 import GrantType0
        from ..models.grant_type_1 import GrantType1
        from ..models.grant_type_3 import GrantType3
        from ..models.grant_type_2 import GrantType2
        from ..models.grant_type_4 import GrantType4

        d = dict(src_dict)
        app_id = d.pop("appId")

        from_ = d.pop("from")

        org = d.pop("org")

        grants = []
        _grants = d.pop("grants")
        for grants_item_data in _grants:

            def _parse_grants_item(
                data: object,
            ) -> Union[
                "GrantType0", "GrantType1", "GrantType2", "GrantType3", "GrantType4"
            ]:
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_grant_type_0 = GrantType0.from_dict(data)

                    return componentsschemas_grant_type_0
                except:  # noqa: E722
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_grant_type_1 = GrantType1.from_dict(data)

                    return componentsschemas_grant_type_1
                except:  # noqa: E722
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_grant_type_2 = GrantType2.from_dict(data)

                    return componentsschemas_grant_type_2
                except:  # noqa: E722
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_grant_type_3 = GrantType3.from_dict(data)

                    return componentsschemas_grant_type_3
                except:  # noqa: E722
                    pass
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_grant_type_4 = GrantType4.from_dict(data)

                return componentsschemas_grant_type_4

            grants_item = _parse_grants_item(grants_item_data)

            grants.append(grants_item)

        created = d.pop("created")

        added = cast(list[str], d.pop("added"))

        revoked_invites = d.pop("revokedInvites")

        app_moved = cls(
            app_id=app_id,
            from_=from_,
            org=org,
            grants=grants,
            created=created,
            added=added,
            revoked_invites=revoked_invites,
        )

        app_moved.additional_properties = d
        return app_moved

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
