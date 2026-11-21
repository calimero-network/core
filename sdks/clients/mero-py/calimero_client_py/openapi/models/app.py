from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.app_components import AppComponents
from ..models.app_executor import AppExecutor
from ..types import UNSET, Unset
from typing import cast
from typing import cast, Union
from typing import Union

if TYPE_CHECKING:
    from ..models.grant_type_0 import GrantType0
    from ..models.grant_type_3 import GrantType3
    from ..models.app_restore import AppRestore
    from ..models.app_links import AppLinks
    from ..models.upgrade import Upgrade
    from ..models.grant_type_2 import GrantType2
    from ..models.package_ref import PackageRef
    from ..models.app_attached import AppAttached
    from ..models.grant_type_4 import GrantType4
    from ..models.grant_type_1 import GrantType1


T = TypeVar("T", bound="App")


@_attrs_define
class App:
    """An app instance (a context) as the directory records it. `executor` is absent for an SDK 0.12 app (wasmer), which
    this node lists but does not run; `release` is the package release it runs, absent for a record from before or a raw
    upload; `restore` is a restore from a backup under way.

        Attributes:
            app_id (str):
            name (str):
            group (int):
            bundle (str):
            owner (str):
            org (Union[None, str]):
            grants (list[Union['GrantType0', 'GrantType1', 'GrantType2', 'GrantType3', 'GrantType4']]):
            confirm_methods (list[str]):
            replicas (list[int]):
            created_at (int):
            init_args (Any): Free-form JSON: its shape is not this API's (see the description around it for whose it is).
            metadata (Any): Free-form JSON: its shape is not this API's (see the description around it for whose it is).
            executor (Union[Unset, AppExecutor]):
            compute_bundle (Union[Unset, str]):
            components (Union[Unset, AppComponents]):
            last_upgrade (Union[Unset, Upgrade]): The last upgrade tried on an app, done or failed: `from` and `to` are
                bundle hashes, `by` the account (or `operator`), `run` the organization-wide run it was part of, `apiKey` the
                upgrade key that made it. `class` and `reasons`: what the update classifier said of it, judged again where it
                applied (absent before cluster version 1.0.0-rc.34, for a raw bundle and a failed upgrade); `approvedBy`: the
                first approver of a red update whose policy asks for a second.
            deployment (Union[Unset, str]):
            release (Union[Unset, PackageRef]): One service of one package release: `service` is absent for a single-service
                bundle, `compute` (absent: false) marks the service's compute component.
            restore (Union[Unset, AppRestore]):
            links (Union[Unset, AppLinks]):
            attached (Union[Unset, AppAttached]):
    """

    app_id: str
    name: str
    group: int
    bundle: str
    owner: str
    org: Union[None, str]
    grants: list[
        Union["GrantType0", "GrantType1", "GrantType2", "GrantType3", "GrantType4"]
    ]
    confirm_methods: list[str]
    replicas: list[int]
    created_at: int
    init_args: Any
    metadata: Any
    executor: Union[Unset, AppExecutor] = UNSET
    compute_bundle: Union[Unset, str] = UNSET
    components: Union[Unset, AppComponents] = UNSET
    last_upgrade: Union[Unset, "Upgrade"] = UNSET
    deployment: Union[Unset, str] = UNSET
    release: Union[Unset, "PackageRef"] = UNSET
    restore: Union[Unset, "AppRestore"] = UNSET
    links: Union[Unset, "AppLinks"] = UNSET
    attached: Union[Unset, "AppAttached"] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.grant_type_0 import GrantType0
        from ..models.grant_type_3 import GrantType3
        from ..models.app_restore import AppRestore
        from ..models.app_links import AppLinks
        from ..models.upgrade import Upgrade
        from ..models.grant_type_2 import GrantType2
        from ..models.package_ref import PackageRef
        from ..models.app_attached import AppAttached
        from ..models.grant_type_4 import GrantType4
        from ..models.grant_type_1 import GrantType1

        app_id = self.app_id

        name = self.name

        group = self.group

        bundle = self.bundle

        owner = self.owner

        org: Union[None, str]
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

        confirm_methods = self.confirm_methods

        replicas = self.replicas

        created_at = self.created_at

        init_args = self.init_args

        metadata = self.metadata

        executor: Union[Unset, str] = UNSET
        if not isinstance(self.executor, Unset):
            executor = self.executor.value

        compute_bundle = self.compute_bundle

        components: Union[Unset, str] = UNSET
        if not isinstance(self.components, Unset):
            components = self.components.value

        last_upgrade: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.last_upgrade, Unset):
            last_upgrade = self.last_upgrade.to_dict()

        deployment = self.deployment

        release: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.release, Unset):
            release = self.release.to_dict()

        restore: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.restore, Unset):
            restore = self.restore.to_dict()

        links: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.links, Unset):
            links = self.links.to_dict()

        attached: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.attached, Unset):
            attached = self.attached.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "appId": app_id,
                "name": name,
                "group": group,
                "bundle": bundle,
                "owner": owner,
                "org": org,
                "grants": grants,
                "confirmMethods": confirm_methods,
                "replicas": replicas,
                "createdAt": created_at,
                "initArgs": init_args,
                "metadata": metadata,
            }
        )
        if executor is not UNSET:
            field_dict["executor"] = executor
        if compute_bundle is not UNSET:
            field_dict["computeBundle"] = compute_bundle
        if components is not UNSET:
            field_dict["components"] = components
        if last_upgrade is not UNSET:
            field_dict["lastUpgrade"] = last_upgrade
        if deployment is not UNSET:
            field_dict["deployment"] = deployment
        if release is not UNSET:
            field_dict["release"] = release
        if restore is not UNSET:
            field_dict["restore"] = restore
        if links is not UNSET:
            field_dict["links"] = links
        if attached is not UNSET:
            field_dict["attached"] = attached

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.grant_type_0 import GrantType0
        from ..models.grant_type_3 import GrantType3
        from ..models.app_restore import AppRestore
        from ..models.app_links import AppLinks
        from ..models.upgrade import Upgrade
        from ..models.grant_type_2 import GrantType2
        from ..models.package_ref import PackageRef
        from ..models.app_attached import AppAttached
        from ..models.grant_type_4 import GrantType4
        from ..models.grant_type_1 import GrantType1

        d = dict(src_dict)
        app_id = d.pop("appId")

        name = d.pop("name")

        group = d.pop("group")

        bundle = d.pop("bundle")

        owner = d.pop("owner")

        def _parse_org(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        org = _parse_org(d.pop("org"))

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

        confirm_methods = cast(list[str], d.pop("confirmMethods"))

        replicas = cast(list[int], d.pop("replicas"))

        created_at = d.pop("createdAt")

        init_args = d.pop("initArgs")

        metadata = d.pop("metadata")

        _executor = d.pop("executor", UNSET)
        executor: Union[Unset, AppExecutor]
        if isinstance(_executor, Unset):
            executor = UNSET
        else:
            executor = AppExecutor(_executor)

        compute_bundle = d.pop("computeBundle", UNSET)

        _components = d.pop("components", UNSET)
        components: Union[Unset, AppComponents]
        if isinstance(_components, Unset):
            components = UNSET
        else:
            components = AppComponents(_components)

        _last_upgrade = d.pop("lastUpgrade", UNSET)
        last_upgrade: Union[Unset, Upgrade]
        if isinstance(_last_upgrade, Unset):
            last_upgrade = UNSET
        else:
            last_upgrade = Upgrade.from_dict(_last_upgrade)

        deployment = d.pop("deployment", UNSET)

        _release = d.pop("release", UNSET)
        release: Union[Unset, PackageRef]
        if isinstance(_release, Unset):
            release = UNSET
        else:
            release = PackageRef.from_dict(_release)

        _restore = d.pop("restore", UNSET)
        restore: Union[Unset, AppRestore]
        if isinstance(_restore, Unset):
            restore = UNSET
        else:
            restore = AppRestore.from_dict(_restore)

        _links = d.pop("links", UNSET)
        links: Union[Unset, AppLinks]
        if isinstance(_links, Unset):
            links = UNSET
        else:
            links = AppLinks.from_dict(_links)

        _attached = d.pop("attached", UNSET)
        attached: Union[Unset, AppAttached]
        if isinstance(_attached, Unset):
            attached = UNSET
        else:
            attached = AppAttached.from_dict(_attached)

        app = cls(
            app_id=app_id,
            name=name,
            group=group,
            bundle=bundle,
            owner=owner,
            org=org,
            grants=grants,
            confirm_methods=confirm_methods,
            replicas=replicas,
            created_at=created_at,
            init_args=init_args,
            metadata=metadata,
            executor=executor,
            compute_bundle=compute_bundle,
            components=components,
            last_upgrade=last_upgrade,
            deployment=deployment,
            release=release,
            restore=restore,
            links=links,
            attached=attached,
        )

        app.additional_properties = d
        return app

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
